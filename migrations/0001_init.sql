
-- Create tables for raw txs and running netflow
CREATE TABLE IF NOT EXISTS raw_transactions (
    tx_hash TEXT PRIMARY KEY,
    block_number INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    to_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    amount TEXT NOT NULL  -- store as decimal string to avoid precision loss
);

CREATE INDEX IF NOT EXISTS idx_raw_from ON raw_transactions(from_address);
CREATE INDEX IF NOT EXISTS idx_raw_to ON raw_transactions(to_address);
CREATE INDEX IF NOT EXISTS idx_raw_token ON raw_transactions(token_address);
CREATE INDEX IF NOT EXISTS idx_raw_block ON raw_transactions(block_number);

CREATE TABLE IF NOT EXISTS state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- optional cache table that keeps totals so far
CREATE TABLE IF NOT EXISTS netflow_totals (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    inflow TEXT NOT NULL DEFAULT '0',
    outflow TEXT NOT NULL DEFAULT '0'
);

INSERT OR IGNORE INTO netflow_totals (id, inflow, outflow) VALUES (1, '0', '0');
