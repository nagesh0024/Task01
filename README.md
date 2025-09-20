
# POL Indexer – Real‑time Polygon Net‑Flow to Binance (Rust)

Indexes **POL** ERC‑20 `Transfer` events on the **Polygon** network in **real time** and tracks
**cumulative net‑flows** (inflow − outflow) to known **Binance** deposit/withdrawal addresses.

> Scope matches the task: real‑time only (no historical backfill).

---

## ✨ Features
- WebSocket **head-following** (no backfill) on Polygon
- Filters **Transfer** logs for the **POL** token (configurable)
- Stores raw transfers in **SQLite**
- Maintains cached **inflow/outflow** totals and exposes a tiny **HTTP API**
- Clean, production‑ready structure with migrations and config

---

## 🧱 Tech
- **Rust** + Tokio
- **ethers-rs** for chain access
- **sqlx (SQLite)** for storage
- **Axum** for `/netflow` endpoint
- Config via `config.yaml` and `.env`

---

## ⚙️ Quickstart

### 1) Install Rust
```bash
rustup default stable
```

### 2) Clone & configure
```bash
git clone <YOUR_REPO_URL>.git
cd pol-indexer
cp .env.example .env
# Edit .env if needed
# Edit config.yaml and set `token_address` to the POL contract on Polygon
```

> **Important:** Set `token_address` to the POL ERC‑20 contract on **Polygon**.

### 3) Run
```bash
cargo run --release
```

The server will bind to `BIND_ADDR` (default `0.0.0.0:8080`).

- Health: `GET /health`
- Netflow: `GET /netflow` → `{"inflow":"...","outflow":"...","cumulative":"..."}` (raw integer units)

### Database
- Default DB: `sqlite://./data/indexer.sqlite`
- Migrations run automatically at startup.

### Binaries / Commands
Single binary runs both the indexer and the HTTP API.

---

## 🗄️ Schema

**raw_transactions**
- `tx_hash` (PK), `block_number`, `timestamp`, `from_address`, `to_address`, `token_address`, `amount` (string; raw integer units)

**netflow_totals**
- Single row cache: `inflow`, `outflow` (strings, raw integer units)

**state**
- `last_block` → last processed block number (string)

> *Rationale:* Store token amounts as **decimal strings** to keep full precision.
Convert to human units on the client side using token `decimals` (e.g., 18).

---

## 🔁 No Backfill
At first launch, we record the current **latest block** and only index new blocks going forward.
If you want to start from a specific block **(still forward-only)**, set `start_from_block` in `config.yaml`.

---

## 🔌 Scaling Plan
- Move from SQLite → **Postgres**
- Replace head‑polling with **`subscribe_logs`** stream and **durable queue** (Kafka/Redis)
- Multi‑exchange by moving addresses to a `exchanges/*.yaml` directory (tag per exchange)
- Partitioned tables and **batched writes** for throughput
- Horizontal indexer workers with **sharded ranges** and a **checkpoint service**

---

## 🔒 Config

`config.yaml`:
```yaml
token_address: "0x...POL_CONTRACT..."         # REQUIRED (Polygon POL contract)
binance_addresses:
  - "0xF977814e90dA44bFA03b6295A0616a897441aceC"
  - "0xe7804c37c13166fF0b37F5aE0BB07A3aEbb6e245"
  - "0x505e71695E9bc45943c58adEC1650577BcA68fD9"
  - "0x290275e3db66394C52272398959845170E4DCb88"
  - "0xD5C08681719445A5Fdce2Bda98b341A49050d821"
  - "0x082489A616aB4D46d1947eE3F912e080815b08DA"
start_from_block: null                         # optional
```

`.env`:
```env
POLYGON_WSS=wss://polygon-rpc.com
DATABASE_URL=sqlite://./data/indexer.sqlite
BIND_ADDR=0.0.0.0:8080
```

---

## 📦 API

### `GET /netflow`
Returns:
```json
{"inflow":"123456789000000000000","outflow":"987000000000000000","cumulative":"122469789000000000000"}
```
> Values are **raw integer** token units. Convert with POL `decimals` (e.g., 18).

---

## 🧪 Notes
- Uses log filtering per block; easy to swap to a streaming `subscribe_logs` if your node supports it stably.
- Idempotency: `INSERT OR IGNORE` on `tx_hash` ensures safe restarts.
- Precision: amounts kept as decimal strings; calculate with `U256` math.

---

## 📝 License
MIT
