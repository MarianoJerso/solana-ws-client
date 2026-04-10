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
// 🌐 CONFIGURACIÓN GLOBAL
// ==============================================================================
const URL_SOLANA_WS: &str = "wss://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
const URL_SOLANA_HTTP: &str = "https://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
// El identificador único de Júpiter Agregator (el "Google Flights" de Solana)
const JUPITER_ID: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

// ==============================================================================
// 🚀 INICIO DEL RUNTIME (MOTOR ASÍNCRONO)
// #[tokio::main] le dice al compilador: "Enchufá el motor de Tokio".
// Internamente levanta un Pool de Threads de sistema operativo y prepara el 
// acceso a 'epoll' del Kernel para eventos de red.
// ==============================================================================
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    // --------------------------------------------------------------------------
    // 🔗 CAPA DE RECURSOS COMPARTIDOS (Arc: Atomic Reference Counter)
    // Inicializamos Reqwest, el cliente HTTP que maneja las conexiones TCP/TLS.
    // Usamos `Arc` porque necesitamos que nuestros 10 Workers usen el mismo cliente 
    // sin hacer copias pesadas en la memoria.
    // --------------------------------------------------------------------------
    let http_client = Arc::new(Client::new());

    // ==============================================================================
    // 🗄️ INICIALIZACIÓN DE LA BASE DE DATOS (SQLITE)
    // ==============================================================================
    println!("Iniciando base de datos...");
    
    // Creamos un POOL de conexiones asincrónicas. Como tenemos múltiples Workers 
    // intentando escribir en disco al mismo tiempo, el Pool organiza y comparte 
    // las conexiones disponibles. SQLite es un archivo único en disco.
    const CANTIDAD_WORKERS: usize = 10;
    
    let pool = SqlitePoolOptions::new()
        // Permitimos tantas conexiones máximas simultáneas como trabajadores tengamos
        .max_connections(CANTIDAD_WORKERS as u32)
        // param 'mode=rwc' -> Read, Write, Create (Crea el archivo si no existe)
        .connect("sqlite:swaps.db?mode=rwc")
        .await?;

    // Ejecutamos SQL crudo para levantar la tabla estructural si es que es la primera 
    // vez que ejecutamos el código. 
    // AUTOINCREMENT asigna automáticamente el ID 1, 2, 3...
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
    println!("Base de datos lista!\n");

    // --------------------------------------------------------------------------
    // 📦 EL TUBO DE COMUNICACIÓN (Canal MPMC)
    // Multi-Producer, Multi-Consumer. Es el "desacople" arquitectónico entre 
    // el WebSocket y los Workers HTTP.
    // tx = Transmisor (WS manda slots)
    // rx = Receptor (Workers consumen slots de la cola)
    // --------------------------------------------------------------------------
    let (tx_slots, rx_slots) = async_channel::bounded::<u64>(500);

    // --------------------------------------------------------------------------
    // ⚙️ EL EJÉRCITO DE CONSUMIDORES (WORKERS)
    // Spawneamos (tiramos a la cola de Tokio) tareas concurrentes.
    // Tokio balanceará estos "Futures" entre sus threads reales.
    // --------------------------------------------------------------------------
    for id_worker in 1..=CANTIDAD_WORKERS {
        
        // Cada trabajador necesita su propia "copia" del tubo, del HTTP, y de la Base de Datos
        let rx_clon = rx_slots.clone();
        let http_clon = Arc::clone(&http_client);
        let pool_clon = pool.clone(); // SqlitePool ya es un Arc por debajo, clone es muy barato

        // `tokio::spawn(async move { ... })` transfiere la propiedad/ownership
        // exclusivamente a esta tarea, previniendo crashes de memoria.
        tokio::spawn(async move {
            println!("  [Worker #{}] En posicion y esperando instrucciones...", id_worker);

            // rx_clon.recv().await: si la cola está vacía, devuelve Poll::Pending
            // y el thread no se bloquea, sino que Tokio lo pasa a dormir.
            while let Ok(numero_slot) = rx_clon.recv().await {
                
                let max_reintentos = 10;
                let mut bloque_encontrado = false;

                // Loop de reintentos
                for intento in 1..=max_reintentos {
                    
                    // Armamos el pedido JSON-RPC 2.0 (RPC = Remote Procedure Call)
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

                    // post().await -> Cruza la red. Devuelve "Pending", libera el Thread.
                    match http_clon.post(URL_SOLANA_HTTP).json(&request_body).send().await {
                        Ok(resp) => {
                            // Llegaron los headers HTTP. Ahora esperamos que bajen los megabytes del bloque
                            if let Ok(block_data) = resp.json::<Value>().await {
                                
                                // ¿El servidor respondió con error (el bloque aún no existe)?
                                if block_data.get("error").is_some() {
                                    // BACKOFF LINEAL: Espera que aumenta para no saturar al servidor
                                    let demora_ms = 500 * intento; 
                                    println!("  [Worker #{}] Slot {} demorado (intento {}/{}). Esperando {}ms...", id_worker, numero_slot, intento, max_reintentos, demora_ms);
                                    
                                    tokio::time::sleep(Duration::from_millis(demora_ms)).await;
                                    continue; 
                                }

                                // Si el bloque existe, procesamos y cortamos el loop de reintentos
                                if let Some(resultado_bloque) = block_data.get("result") {
                                    procesar_y_guardar_bloque(numero_slot, resultado_bloque, id_worker, pool_clon.clone()).await;
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

    // ==============================================================================
    // 🔊 EL PRODUCTOR PRINCIPAL (WEBSOCKET)
    // El hilo principal se dedica exclusivamente a escuchar la red.
    // ==============================================================================
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
        
        // Separamos el tubo bidireccional en dos (Ownership borrow rules de Rust)
        let (mut write, mut read) = ws_stream.split();

        // Le avisamos al nodo que queremos notificaciones "ligeras" de slots
        let mensaje_suscripcion = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "slotSubscribe"
        });

        if let Err(e) = write.send(Message::Text(mensaje_suscripcion.to_string())).await {
            println!("Error enviando suscripcion: {}", e);
            continue;
        }

        // 🚨 Loop de escaneo ultrarrápido
        loop {
            // El thread asincrónico duerme hasta que "epoll" avise que llegaron bytes por la red
            if let Some(respuesta) = read.next().await {
                match respuesta {
                    Ok(mensaje) => {
                        if let Message::Text(texto) = mensaje {
                            let json_parseado: Result<Value, _> = serde_json::from_str(&texto);
                            if let Ok(datos) = json_parseado {
                                // Navega el JSON directo para descartar payload inútil
                                if let Some(slot_obj) = datos.pointer("/params/result/slot") {
                                    if let Some(numero_slot) = slot_obj.as_u64() {
                                        
                                        println!("🔔 [WS Rápido] Nuevo Slot detectado: {}", numero_slot);
                                        
                                        // Empujar slot al 'buffer' MPMC.
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

// ==============================================================================
// 📊 LOGICA DE PROCESAMIENTO (DATA INGESTION) Y COMPRESIÓN
// ==============================================================================
async fn procesar_y_guardar_bloque(numero_slot: u64, resultado_bloque: &Value, id_worker: usize, db_pool: sqlx::SqlitePool) {
    
    // Parseo de Unix Epoch a fecha humana.
    let unix_time = resultado_bloque["blockTime"].as_i64().unwrap_or(0);
    let datetime: DateTime<Utc> = Utc.timestamp_opt(unix_time, 0).unwrap();
    
    // Extracción de Cantidad con |closure| seguro
    let tx_count = resultado_bloque["transactions"].as_array().map(|arr| arr.len()).unwrap_or(0);
    let mut swaps_encontrados = 0;

    if let Some(transacciones) = resultado_bloque["transactions"].as_array() {
        for tx in transacciones {
            
            // HACK DE COMPRESIÓN DE SOLANA:
            // Solana indexa a los participantes en 'accountKeys'. Las instrucciones
            // individuales solo guardan un número de índice hacia esta lista.
            let account_keys = &tx["transaction"]["message"]["accountKeys"];

            if let Some(instrucciones) = tx["transaction"]["message"]["instructions"].as_array() {
                for instruccion in instrucciones {
                    if let Some(idx) = instruccion["programIdIndex"].as_u64() {
                        
                        // Rescatamos del ELENCO al participante usando el índice numérico
                        let program_id = account_keys[idx as usize].as_str().unwrap_or("");
                        if program_id == JUPITER_ID {
                            swaps_encontrados += 1;
                        }
                    }
                }
            }
        }
    }

    // ==============================================================================
    // 💾 INSERCIÓN EN BASE DE DATOS (ASYNC SQL)
    // ==============================================================================
    // En lugar de escribir a un archivo de texto, insertamos de manera atómica (segura) 
    // en nuestra base de datos.
    // Usamos '?' como place-holders para evitar Inyecciones SQL, y usamos el encadenamiento 
    // de métodos .bind() para inyectar nuestras variables en esos signos de interrogación.
    let result = sqlx::query(
        "INSERT INTO swaps (slot_number, unix_timestamp, tx_count, jupiter_swaps) VALUES (?, ?, ?, ?)"
    )
    .bind(numero_slot as i64)         // ?1
    .bind(unix_time)                  // ?2
    .bind(tx_count as i64)            // ?3
    .bind(swaps_encontrados as i64)   // ?4
    .execute(&db_pool)                // Pasamos la prestada referencia al Pool de conexiones
    .await;                           // Await: Guardar a disco es lento. El thread se libera.

    // Comprobamos si la inserción (Query) fue un éxito o tiró un Error
    match result {
        Ok(_) => {
            let linea_resultado = format!("[Worker #{}] Slot: {} | {} | Total Txs: {} | Swaps Jupiter: {}\n",
                id_worker,
                numero_slot,
                datetime.format("%Y-%m-%d %H:%M:%S"),
                tx_count,
                swaps_encontrados
            );
            print!("  ✅ ¡ÉXITO (BD)! | {}", linea_resultado);
        },
        Err(e) => {
            println!("Error guardando en BD (Slot {}): {}", numero_slot, e);
        }
    }
}
