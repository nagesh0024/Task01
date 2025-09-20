
use std::{collections::HashSet, str::FromStr, sync::Arc};
use anyhow::{Context, Result};
use axum::{routing::get, Router, Json};
use dotenvy::dotenv;
use ethers::{
    prelude::*,
    types::{Filter, H160, H256, Log, U256, BlockNumber},
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_yaml;
use sqlx::{SqlitePool, Sqlite, migrate::MigrateDatabase};
use tokio::{sync::RwLock, task::JoinSet};
use tracing::{info, warn, error};
use tracing_subscriber::{EnvFilter, fmt::Subscriber};

static TRANSFER_TOPIC: Lazy<H256> = Lazy::new(|| H256::from_slice(
    // keccak256("Transfer(address,address,uint256)")
    &hex_literal::hex!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef")
));

#[derive(Debug, Deserialize)]
struct AppConfig {
    token_address: String,
    binance_addresses: Vec<String>,
    #[serde(default)]
    start_from_block: Option<u64>,
}

#[derive(Debug, Default, Serialize)]
struct Netflow {
    inflow: String,
    outflow: String,
    cumulative: String,
}

#[tokio::main(flavor="multi_thread")]
async fn main() -> Result<()> {
    dotenv().ok();
    let subscriber = Subscriber::builder().with_env_filter(EnvFilter::from_default_env()).finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://./data/indexer.sqlite".into());
    if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
        Sqlite::create_database(&db_url).await?;
    }
    let pool = SqlitePool::connect(&db_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Read config
    let cfg_text = std::fs::read_to_string("config.yaml").context("missing config.yaml")?;
    let cfg: AppConfig = serde_yaml::from_str(&cfg_text).context("invalid config.yaml")?;

    // Normalize addresses to lowercase
    let token = normalize(&cfg.token_address);
    let binance: HashSet<String> = cfg.binance_addresses.into_iter().map(normalize).collect();

    // Provider
    let wss = std::env::var("POLYGON_WSS").unwrap_or_else(|_| "wss://polygon-rpc.com".into());
    let ws = Ws::connect(wss.clone()).await.context("WS connect failed")?;
    let provider = Provider::new(ws).interval(std::time::Duration::from_millis(200));
    let provider = Arc::new(provider);

    // If start_from_block is provided, store it into state; else set to latest (no backfill)
    if let Some(start) = cfg.start_from_block {
        sqlx::query("INSERT OR REPLACE INTO state (key, value) VALUES ('last_block', ?)")
            .bind(start.to_string()).execute(&pool).await?;
    } else {
        let latest: U64 = provider.get_block_number().await?;
        sqlx::query("INSERT OR REPLACE INTO state (key, value) VALUES ('last_block', ?)")
            .bind(latest.as_u64().to_string()).execute(&pool).await?;
    }

    let net = Arc::new(RwLock::new(Netflow::default()));
    // Preload cached totals
    {
        if let Ok(rec) = sqlx::query_as::<_, (String, String)>("SELECT inflow, outflow FROM netflow_totals WHERE id=1").fetch_one(&pool).await {
            let (inflow, outflow) = rec;
            let cumulative = sub_decimal(&inflow, &outflow);
            *net.write().await = Netflow{ inflow, outflow, cumulative };
        }
    }

    // HTTP server
    let http_pool = pool.clone();
    let http_net = net.clone();
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/netflow", get(move || {
            let net = http_net.clone();
            async move {
                let current = net.read().await.clone();
                Json(current)
            }
        }));

    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    info!("HTTP server listening on http://{}", bind);

    // Spawn indexer
    let indexer = tokio::spawn(run_indexer(provider.clone(), pool.clone(), token.clone(), binance.clone(), net.clone()));

    // Serve
    axum::serve(listener, app).await?;
    indexer.await??;
    Ok(())
}

async fn run_indexer(provider: Arc<Provider<Ws>>, pool: SqlitePool, token: String, binance: HashSet<String>, net: Arc<RwLock<Netflow>>) -> Result<()> {
    info!("Starting indexer…");
    let mut set = JoinSet::new();

    // Poll-head loop: subscribe to new blocks and immediately filter logs for token Transfer events in that block
    loop {
        let latest: U64 = provider.get_block_number().await?;
        let last: u64 = sqlx::query_scalar::<_, String>("SELECT value FROM state WHERE key='last_block'")
            .fetch_optional(&pool).await?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(latest.as_u64());

        if latest.as_u64() > last {
            // process each new block once
            for b in (last+1)..=latest.as_u64() {
                if let Err(e) = process_block(&provider, &pool, &token, &binance, b, &net).await {
                    error!("process_block {} failed: {:?}", b, e);
                } else {
                    sqlx::query("INSERT OR REPLACE INTO state (key, value) VALUES ('last_block', ?)")
                        .bind(b.to_string()).execute(&pool).await?;
                }
            }
        }

        // sleep briefly
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    }
}

async fn process_block(provider: &Provider<Ws>, pool: &SqlitePool, token: &str, binance: &HashSet<String>, block_number: u64, net: &Arc<RwLock<Netflow>>) -> Result<()> {
    // Build a filter for the token Transfer topic in this block
    let token_addr = H160::from_str(token).context("bad token address")?;
    let filter = Filter::new()
        .address(token_addr)
        .from_block(BlockNumber::Number(block_number.into()))
        .to_block(BlockNumber::Number(block_number.into()))
        .topic0(*TRANSFER_TOPIC);

    let logs: Vec<Log> = provider.get_logs(&filter).await?;
    if logs.is_empty() {
        return Ok(());
    }

    // Prepare a tx
    let mut tx = pool.begin().await?;

    for lg in logs {
        // ERC20 Transfer has topics: [transfer, from, to]; data = amount (uint256)
        if lg.topics.len() < 3 { continue; }
        let from = topic_to_addr(&lg.topics[1]);
        let to = topic_to_addr(&lg.topics[2]);
        let amount = U256::from_big_endian(&lg.data.to_vec());

        // Fetch block timestamp if available
        let ts = if let Some(bn) = lg.block_number {
            if let Some(block) = provider.get_block(bn).await? {
                block.timestamp.as_u64() as i64
            } else { 0 }
        } else { 0 };

        // Save raw
        let hash = lg.transaction_hash.unwrap_or_default();
        let tx_hash = format!("{:#x}", hash);
        let block_num = lg.block_number.unwrap_or_default().as_u64() as i64;
        let token_s = normalize(&format!("{:#x}", lg.address));
        let from_s = normalize(&from);
        let to_s = normalize(&to);
        let amount_dec = u256_to_decimal_string(amount);

        sqlx::query(r#"
            INSERT OR IGNORE INTO raw_transactions
                (tx_hash, block_number, timestamp, from_address, to_address, token_address, amount)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#)
            .bind(&tx_hash)
            .bind(block_num)
            .bind(ts)
            .bind(&from_s)
            .bind(&to_s)
            .bind(&token_s)
            .bind(&amount_dec)
            .execute(&mut *tx).await?;

        // Update netflow totals when Binance is involved
        let mut inflow = None;
        let mut outflow = None;
        if binance.contains(&to_s) {
            inflow = Some(amount_dec.clone());
        }
        if binance.contains(&from_s) {
            outflow = Some(amount_dec.clone());
        }
        if inflow.is_some() || outflow.is_some() {
            // Read current
            let (cur_in, cur_out): (String, String) = sqlx::query_as("SELECT inflow, outflow FROM netflow_totals WHERE id=1")
                .fetch_one(&mut *tx).await?;
            let new_in = add_decimal(&cur_in, inflow.as_deref().unwrap_or("0"));
            let new_out = add_decimal(&cur_out, outflow.as_deref().unwrap_or("0"));
            sqlx::query("UPDATE netflow_totals SET inflow=?, outflow=? WHERE id=1")
                .bind(&new_in)
                .bind(&new_out)
                .execute(&mut *tx).await?;

            let cumulative = sub_decimal(&new_in, &new_out);
            // Update in-memory snapshot for HTTP
            {
                let mut guard = net.write().await;
                guard.inflow = new_in;
                guard.outflow = new_out;
                guard.cumulative = cumulative;
            }
        }
    }

    tx.commit().await?;
    Ok(())
}

// Helpers
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

fn topic_to_addr(t: &H256) -> String {
    // rightmost 20 bytes of the 32-byte topic
    let v = t.as_bytes();
    let addr = &v[12..];
    format!("0x{}", hex::encode(addr))
}

fn u256_to_decimal_string(v: U256) -> String {
    // Return raw integer as string (no decimals applied). Consumers may divide by token decimals.
    v.to_string()
}

// Naive decimal add/sub using big integers via U256 parsing from string
fn add_decimal(a: &str, b: &str) -> String {
    let ua = U256::from_dec_str(a).unwrap_or(U256::ZERO);
    let ub = U256::from_dec_str(b).unwrap_or(U256::ZERO);
    (ua + ub).to_string()
}

fn sub_decimal(a: &str, b: &str) -> String {
    let ua = U256::from_dec_str(a).unwrap_or(U256::ZERO);
    let ub = U256::from_dec_str(b).unwrap_or(U256::ZERO);
    if ua >= ub { (ua - ub).to_string() } else { "0".into() }
}
