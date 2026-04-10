use chrono::{DateTime, Utc, TimeZone};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use std::time::Duration;
use std::sync::Arc;
use sqlx::sqlite::SqlitePoolOptions;

// ==============================================================================
// GLOBAL CONFIGURATION
// ==============================================================================
const URL_SOLANA_WS: &str = "wss://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
const URL_SOLANA_HTTP: &str = "https://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";

// Jupiter Aggregator Program ID
const JUPITER_ID: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

// ==============================================================================
// RUNTIME ENTRY_POINT
// ==============================================================================
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    // --------------------------------------------------------------------------
    // SHARED RESOURCES (Arc)
    // --------------------------------------------------------------------------
    let http_client = Arc::new(Client::new());

    // --------------------------------------------------------------------------
    // DATABASE INITIALIZATION (SQLite)
    // --------------------------------------------------------------------------
    println!("[INFO] Initializing SQLite database...");
    
    const WORKER_COUNT: usize = 10;
    
    let pool = SqlitePoolOptions::new()
        .max_connections(WORKER_COUNT as u32)
        .connect("sqlite:swaps.db?mode=rwc")
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS swaps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            slot_number INTEGER NOT NULL,
            unix_timestamp INTEGER NOT NULL,
            tx_count INTEGER NOT NULL,
            jupiter_swaps INTEGER NOT NULL
        )"
    )
    .execute(&pool)
    .await?;

    println!("[INFO] Database successfully initialized.\n");

    // --------------------------------------------------------------------------
    // MPMC CHANNEL (Message Buffer)
    // --------------------------------------------------------------------------
    let (tx_slots, rx_slots) = async_channel::bounded::<u64>(500);

    // --------------------------------------------------------------------------
    // ASYNC HTTP WORKERS POOL
    // --------------------------------------------------------------------------
    for worker_id in 1..=WORKER_COUNT {
        
        let rx_clone = rx_slots.clone();
        let http_clone = Arc::clone(&http_client);
        let pool_clone = pool.clone();

        tokio::spawn(async move {
            println!("[Worker #{}] Spun up and awaiting tasks...", worker_id);

            while let Ok(slot_number) = rx_clone.recv().await {
                
                let max_retries = 10;
                let mut block_found = false;

                // Linear backoff retry loop
                for attempt in 1..=max_retries {
                    
                    let request_body = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "getBlock",
                        "params": [
                            slot_number,
                            {
                               "encoding": "json",
                               "transactionDetails": "full",
                               "rewards": false,
                               "maxSupportedTransactionVersion": 0
                            }
                        ]
                    });

                    match http_clone.post(URL_SOLANA_HTTP).json(&request_body).send().await {
                        Ok(resp) => {
                            if let Ok(block_data) = resp.json::<Value>().await {
                                
                                // Handle RPC node synchronization delays (Block not available yet)
                                if block_data.get("error").is_some() {
                                    let backoff_ms = 500 * attempt; 
                                    println!("[Worker #{}] Slot {} delayed (attempt {}/{}). Sleeping {}ms...", worker_id, slot_number, attempt, max_retries, backoff_ms);
                                    
                                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                                    continue; 
                                }

                                // Successful block retrieval
                                if let Some(block_result) = block_data.get("result") {
                                    process_and_store_block(slot_number, block_result, worker_id, pool_clone.clone()).await;
                                    block_found = true;
                                    break;
                                }
                            }
                        },
                        Err(_) => {
                            println!("[Worker #{}] Network error fetching slot {}. Retrying...", worker_id, slot_number);
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                        }
                    }
                }

                if !block_found {
                    println!("[ERROR] Worker #{} failed to retrieve slot {} after {} attempts. Skipping.", worker_id, slot_number, max_retries);
                }
            }
        });
    }

    // ==============================================================================
    // MAIN THREAD: WEBSOCKET PRODUCER
    // ==============================================================================
    loop {
        println!("[INFO] Attempting to connect to Chainstack WebSocket...");

        let url = Url::parse(URL_SOLANA_WS)?;
        
        let (ws_stream, _) = match connect_async(url).await {
            Ok(s) => s,
            Err(e) => {
                println!("[ERROR] WS connection failed, retrying in 3 seconds: {}", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        println!("[INFO] Connected successfully. Transmitting slot subscription payload...\n");
        
        let (mut write, mut read) = ws_stream.split();

        let subscription_payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "slotSubscribe"
        });

        if let Err(e) = write.send(Message::Text(subscription_payload.to_string())).await {
            println!("[ERROR] Failed to send subscription payload: {}", e);
            continue;
        }

        // Fast Event Loop
        loop {
            if let Some(response) = read.next().await {
                match response {
                    Ok(message) => {
                        if let Message::Text(text) = message {
                            let parsed_json: Result<Value, _> = serde_json::from_str(&text);
                            if let Ok(data) = parsed_json {
                                
                                // Pointer extraction to mitigate full document deserialization overhead
                                if let Some(slot_obj) = data.pointer("/params/result/slot") {
                                    if let Some(slot_number) = slot_obj.as_u64() {
                                        
                                        println!("[WS] Slot notification received: {}", slot_number);
                                        
                                        // Push to MPMC channel queue
                                        if let Err(e) = tx_slots.send(slot_number).await {
                                            println!("[ERROR] Failed to push slot to channel: {}", e);
                                        }
                                        
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("[ERROR] WebSocket disconnected: {}. Attempting reconnect...\n", e);
                        break; 
                    }
                }
            }
        }
    }
}

// ==============================================================================
// BLOCK PROCESSING (DATA INGESTION)
// ==============================================================================
async fn process_and_store_block(slot_number: u64, block_result: &Value, worker_id: usize, db_pool: sqlx::SqlitePool) {
    
    // Parse Unix Epoch to human-readable datetime
    let unix_time = block_result["blockTime"].as_i64().unwrap_or(0);
    let datetime: DateTime<Utc> = Utc.timestamp_opt(unix_time, 0).unwrap();
    
    let tx_count = block_result["transactions"].as_array().map(|arr| arr.len()).unwrap_or(0);
    let mut jupiter_swaps_found = 0;

    if let Some(transactions) = block_result["transactions"].as_array() {
        for tx in transactions {
            
            // Extract the global account keys referenced by indexed pointers within this transaction
            let account_keys = &tx["transaction"]["message"]["accountKeys"];

            if let Some(instructions) = tx["transaction"]["message"]["instructions"].as_array() {
                for instruction in instructions {
                    if let Some(idx) = instruction["programIdIndex"].as_u64() {
                        
                        // Resolve the index pointer against the accountKeys array
                        let program_id = account_keys[idx as usize].as_str().unwrap_or("");
                        
                        if program_id == JUPITER_ID {
                            jupiter_swaps_found += 1;
                        }
                    }
                }
            }
        }
    }

    // ==============================================================================
    // SQLITE ASYNC INSERTION
    // ==============================================================================
    let query_result = sqlx::query(
        "INSERT INTO swaps (slot_number, unix_timestamp, tx_count, jupiter_swaps) VALUES (?, ?, ?, ?)"
    )
    .bind(slot_number as i64)
    .bind(unix_time)
    .bind(tx_count as i64)
    .bind(jupiter_swaps_found as i64)
    .execute(&db_pool)
    .await;

    match query_result {
        Ok(_) => {
            let output_line = format!("[Worker #{}] Slot: {} | {} | Total Txs: {} | Jupiter Swaps: {}\n",
                worker_id,
                slot_number,
                datetime.format("%Y-%m-%d %H:%M:%S"),
                tx_count,
                jupiter_swaps_found
            );
            print!("[SUCCESS] {}", output_line);
        },
        Err(e) => {
            println!("[ERROR] Failed to persist data for slot {}: {}", slot_number, e);
        }
    }
}
