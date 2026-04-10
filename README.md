# solana-ws-client

[Leer en Español](README.es.md)

`solana-ws-client` is an asynchronous data ingestion node for the Solana blockchain. It employs an event-driven architecture to index on-chain activity in real-time, currently focusing on automated routing volume.

## Architecture

The system decouples notification streaming from payload fetching to mitigate head-of-line blocking:

1. **Notification Layer**: A WebSocket connection subscribes to Solana slot notifications (`slotSubscribe`) to maintain a low-latency event stream.
2. **MPMC Channel**: A bounded multi-producer, multi-consumer channel buffers incoming slot notifications, providing backpressure.
3. **Async Workers**: A configurable pool of Tokio-based workers pull from the channel, executing JSON-RPC POST requests to retrieve full block data.
4. **Persistence**: Extracted block metrics and transaction aggregates are asynchronously written to a local SQLite database using `sqlx`.

## Technical Details

- **Runtime**: Utilizes `tokio` for non-blocking I/O and task scheduling.
- **Concurrency**: State is shared across workers via `Arc` (reqwest connection pools, sqlx database pools) to avoid memory duplication.
- **Reliability**: Network requests implement a linear backoff strategy to handle rate limits and block propagation delays gracefully.
- **Parsing**: Direct inspection of the `accountKeys` array and transaction instruction pointers (`programIdIndex`) minimizes allocation overhead when identifying target smart contracts (e.g., Jupiter Aggregator).

## Usage

### Dependencies

- Rust (Edition 2024 or later)

### Build and Run

```bash
git clone https://github.com/MarianoJerso/solana-ws-client.git
cd solana-ws-client
cargo run --release
```

Upon initialization, the application will create a `swaps.db` SQLite database file in the root directory and begin indexing block data automatically.
