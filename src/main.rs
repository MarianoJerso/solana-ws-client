use chrono::{DateTime, Utc, TimeZone};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use std::time::Duration;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

const URL_SOLANA_WS: &str = "wss://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
const URL_SOLANA_HTTP: &str = "https://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
const JUPITER_ID: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Iniciamos el cliente HTTP envuelto en un Arc (Atomic Reference Counter)
    // Esto permite que el cliente sea compartido de forma segura entre todos los clones/trabajadores
    let http_client = Arc::new(Client::new());

    // 2. CREAMOS EL CANAL MPMC (Multi-Producer, Multi-Consumer)
    // Usamos `async_channel` porque el mpsc nativo de Tokio no permite clonar el receptor (rx)
    // Dejamos 500 espacios en el tubo por las dudas
    let (tx_slots, rx_slots) = async_channel::bounded::<u64>(500);

    // 3. SPAWNEAMOS UN EJÉRCITO DE 10 TRABAJADORES (WORKERS)
    // Si un bloque está demorado y un trabajador se queda esperando 5 segundos,
    // los otros 9 trabajadores siguen procesando los nuevos slots que van cayendo
    const CANTIDAD_WORKERS: usize = 10;
    
    for id_worker in 1..=CANTIDAD_WORKERS {
        
        // Cada trabajador necesita su propia "copia" del tubo y del cliente HTTP
        let rx_clon = rx_slots.clone();
        let http_clon = Arc::clone(&http_client);

        tokio::spawn(async move {
            println!("  [Worker #{}] En posicion y esperando instrucciones...", id_worker);

            // Cada trabajador chupa de la misma manguera compartida
            while let Ok(numero_slot) = rx_clon.recv().await {
                
                let max_reintentos = 5;
                let mut bloque_encontrado = false;

                for intento in 1..=max_reintentos {
                    let request_body = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "getBlock",
                        "params": [
                            numero_slot,
                            {
                               "encoding": "json",
                               "transactionDetails": "full",
                               "rewards": false,
                               "maxSupportedTransactionVersion": 0
                            }
                        ]
                    });

                    match http_clon.post(URL_SOLANA_HTTP).json(&request_body).send().await {
                        Ok(resp) => {
                            if let Ok(block_data) = resp.json::<Value>().await {
                                
                                if block_data.get("error").is_some() {
                                    let demora_ms = 500 * intento; 
                                    println!("  [Worker #{}] Slot {} demorado (intento {}/{}). Esperando {}ms...", id_worker, numero_slot, intento, max_reintentos, demora_ms);
                                    tokio::time::sleep(Duration::from_millis(demora_ms)).await;
                                    continue; 
                                }

                                if let Some(resultado_bloque) = block_data.get("result") {
                                    procesar_y_guardar_bloque(numero_slot, resultado_bloque, id_worker);
                                    bloque_encontrado = true;
                                    break;
                                }
                            }
                        },
                        Err(_) => {
                            println!("  [Worker #{}] Error de red en slot {}. Reintentando...", id_worker, numero_slot);
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                        }
                    }
                }

                if !bloque_encontrado {
                    println!("❌ [Worker #{}] Slot Skippeado irrecuperable: {}", id_worker, numero_slot);
                }
            }
        });
    }

    // 4. EL LOOP PRINCIPAL (PRODUCTOR / WEBSOCKET)
    // El trabajo de este loop es UNICAMENTE escuchar los dings rapidisimos del websocket.
    loop {
        println!("Intentando conectar al WebSocket de Chainstack...");

        let url = Url::parse(URL_SOLANA_WS)?;
        
        let (ws_stream, _) = match connect_async(url).await {
            Ok(s) => s,
            Err(e) => {
                println!("Error WS, reintentando en 3 segundos... ({})", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        println!("Conectado! Enviando suscripcion de SLOTS...\n");
        let (mut write, mut read) = ws_stream.split();

        let mensaje_suscripcion = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "slotSubscribe"
        });

        if let Err(e) = write.send(Message::Text(mensaje_suscripcion.to_string())).await {
            println!("Error enviando suscripcion: {}", e);
            continue;
        }

        // Leemos cada mensaje del websocket
        loop {
            if let Some(respuesta) = read.next().await {
                match respuesta {
                    Ok(mensaje) => {
                        if let Message::Text(texto) = mensaje {
                            let json_parseado: Result<Value, _> = serde_json::from_str(&texto);
                            if let Ok(datos) = json_parseado {
                                if let Some(slot_obj) = datos.pointer("/params/result/slot") {
                                    if let Some(numero_slot) = slot_obj.as_u64() {
                                        
                                        println!("🔔 [WS Rápido] Nuevo Slot detectado: {}", numero_slot);
                                        // TIRAMOS EL SLOT POR EL TUBO
                                        // Si el tubo está lleno, le damos respiro. Si no, lo mandamos al Trabajador
                                        if let Err(e) = tx_slots.send(numero_slot).await {
                                            println!("Error tirando slot al canal: {}", e);
                                        }
                                        
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("WebSocket cortado: {}. Reconectando...\n", e);
                        break; 
                    }
                }
            }
        }
    }
}

// Factorizamos la logica ruidosa de iterar transacciones en una funcion de ayuda pura
fn procesar_y_guardar_bloque(numero_slot: u64, resultado_bloque: &Value, id_worker: usize) {
    let unix_time = resultado_bloque["blockTime"].as_i64().unwrap_or(0);
    let datetime: DateTime<Utc> = Utc.timestamp_opt(unix_time, 0).unwrap();
    
    let tx_count = resultado_bloque["transactions"].as_array().map(|arr| arr.len()).unwrap_or(0);
    let mut swaps_encontrados = 0;

    if let Some(transacciones) = resultado_bloque["transactions"].as_array() {
        for tx in transacciones {
            let account_keys = &tx["transaction"]["message"]["accountKeys"];

            if let Some(instrucciones) = tx["transaction"]["message"]["instructions"].as_array() {
                for instruccion in instrucciones {
                    if let Some(idx) = instruccion["programIdIndex"].as_u64() {
                        let program_id = account_keys[idx as usize].as_str().unwrap_or("");
                        if program_id == JUPITER_ID {
                            swaps_encontrados += 1;
                        }
                    }
                }
            }
        }
    }

    let linea_resultado = format!("[Worker #{}] Slot: {} | {} | Total Txs: {} | Swaps Jupiter: {}\n",
        id_worker,
        numero_slot,
        datetime.format("%Y-%m-%d %H:%M:%S"),
        tx_count,
        swaps_encontrados
    );

    print!("  ✅ ¡ÉXITO! | {}", linea_resultado);

    let mut archivo = OpenOptions::new()
        .create(true)
        .append(true)
        .open("jupiter_swaps.txt")
        .expect("No se pudo abrir el archivo para guardar");

    if let Err(e) = write!(archivo, "{}", linea_resultado) {
        println!("Error escribiendo en el archivo: {}", e);
    }
}
