# 🦀 Solana Data Ingestion Node

![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)
![Solana](https://img.shields.io/badge/Solana-14F195?style=for-the-badge&logo=solana&logoColor=white)
![SQLite](https://img.shields.io/badge/SQLite-07405E?style=for-the-badge&logo=sqlite&logoColor=white)

*[🇪🇸 Leer en Español](README.es.md)*

A high-performance, asynchronous data ingestion node for the Solana blockchain. Built purely in Rust, this system demonstrates advanced distributed system concepts, memory-safe concurrency, and event-driven architecture to index decentralized exchange (DEX) activity in real-time.

---

## 🏗️ Architecture Design (Event-Driven Fetching)

This project avoids the classic pitfalls of Head-of-Line blocking and API rate limit spamming by separating the notification layer from the data-fetching layer:

1. **Lightweight Notification Layer (Consumer):** A WebSocket connection subscribes to slot notifications. It listens for tiny packet pings ("new slot X") and immediately pushes them to an internal buffer.
2. **Decoupled Buffer (MPMC Channel):** A Bounded MPMC (Multi-Producer Multi-Consumer) channel safely acts as a shock absorber between the fast WebSocket and the slower HTTP network operations.
3. **Heavy Lifting Layer (Async Workers):** A pool of 10 concurrent HTTP workers continuously drains the MPMC channel. They execute JSON-RPC POST requests to fetch full block data (megabytes in size), parsing thousands of transactions per block simultaneously.

## 🧠 Key Systems Engineering Concepts

This repository serves as a showcase of low-level and systems-level programming:

*   **Non-Blocking I/O (`tokio` & epoll):** Complete avoidance of OS thread-blocking. Workers actively yield to the `tokio` runtime via futures when waiting for TCP/TLS handshakes, HTTP responses, or Disk I/O.
*   **Memory Safe Concurrency (`Arc<T>`):** Utilizing Atomic Reference Counters to safely share `reqwest` connection pools and `sqlx` database connection pools across all concurrent workers without requiring expensive memory duplication or locking mechanisms.
*   **Fault Tolerance (Backoff Retry):** Network calls use a linear backoff algorithm (500ms * attempt) to smoothly recover from dropped slots or delayed block finality from the RPC node without crashing.
*   **Protocol Parsing & Zero-Copy-ish Extraction:** Direct traversal of deep JSON hierarchies (e.g., extracting Solana's `accountKeys` arrays and resolving `programIdIndex` pointers) to identify specific Jupiter Aggregator swaps.
*   **Asynchronous Database Persistence:** Data is seamlessly written to an on-disk SQLite relational database utilizing `sqlx` macros, ensuring thread-safe inserts via `await` without halting the networking loop.

## 🚀 Quick Start

### Prerequisites
- [Rust](https://www.rust-lang.org/) (Edition 2024)
- A local connection or internet access to fetch the required crates.

### Installation & Execution
```bash
# Clone the repository
git clone https://github.com/MarianoJerso/solana-ws-client.git
cd solana-ws-client

# Run the node
cargo run
```

Upon execution, the system will automatically create `swaps.db` and begin indexing live blockchain blocks.

## 📊 Data Extraction (Jupiter Routing)

The database currently focuses on quantifying the trading volume routed through **Jupiter Aggregator**, the leading DEX on Solana. For every block, it persists:
- `slot_number`: The blockchain slot.
- `unix_timestamp`: Unformatted time-series data for quantitative analysis.
- `tx_count`: Total transactions in the block.
- `jupiter_swaps`: Precise count of transactions hitting the Jupiter smart contract.

*This data serves as the foundational infrastructure layer for MEV detection, wholesale quantitative analysis, or algorithmic trading dashboarding.*
