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
// Internamente levanta un Pool de Threads (generalmente uno por núcleo de tu CPU) 
// y prepara el acceso a 'epoll' del Kernel (Ring 0) para eventos de red.
// ==============================================================================
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    // --------------------------------------------------------------------------
    // 🔗 CAPA DE RECURSOS COMPARTIDOS (Arc: Atomic Reference Counter)
    // Inicializamos Reqwest, el cliente HTTP que maneja las conexiones TCP/TLS (HTTPS).
    // Usamos `Arc` porque necesitamos que nuestros 10 Workers usen el mismo cliente 
    // sin crear 10 copias en la memoria. Arc funciona sumando un contador (+1) cada 
    // vez que le prestamos uso a un Worker, liberando la memoria cuando llega a 0.
    // --------------------------------------------------------------------------
    let http_client = Arc::new(Client::new());

    // --------------------------------------------------------------------------
    // 📦 EL TUBO DE COMUNICACIÓN (Canal MPMC)
    // Multi-Producer, Multi-Consumer. Permite que múltiples hilos escriban y lean 
    // sin pisarse. Es el "desacople" arquitectónico entre el WebSocket y los Workers.
    // Usamos `bounded(500)` como medida de seguridad: si los Workers están muy 
    // lentos, el tubo guarda hasta 500 tareas máximo. Si se llena, bloquea la entrada.
    // tx = Transmisor (quien manda los slots)
    // rx = Receptor (los Workers que sacan los slots del tubo, en orden FIFO)
    // --------------------------------------------------------------------------
    let (tx_slots, rx_slots) = async_channel::bounded::<u64>(500);

    // --------------------------------------------------------------------------
    // ⚙️ EL EJÉRCITO DE CONSUMIDORES (WORKERS)
    // Spawneamos (tiramos a la cola de Tokio) 10 tareas concurrentes.
    // Tokio se va a encargar de balancear estos "Futures" entre sus threads reales.
    // --------------------------------------------------------------------------
    const CANTIDAD_WORKERS: usize = 10;
    
    for id_worker in 1..=CANTIDAD_WORKERS {
        
        // Cada trabajador necesita de su lado del "tubo" y acceso al pool HTTP
        let rx_clon = rx_slots.clone();
        let http_clon = Arc::clone(&http_client);

        // `tokio::spawn(async move { ... })`
        // 1. `async`: Convierte todo este bloque en un `Future` (una máquina de estados).
        // 2. `spawn`: Le dice a Tokio "mandalo a procesarse en background".
        // 3. `move`: Le transfiere la propiedad/ownership de `rx_clon`, `http_clon` 
        //    e `id_worker` EXCLUSIVAMENTE a esta tarea. Así Rust se asegura que la RAM 
        //    no va a ser accedida crasheando el sistema.
        tokio::spawn(async move {
            println!("  [Worker #{}] En posicion y esperando instrucciones...", id_worker);

            // rx_clon.recv().await: 
            // Esto devuelve un `Poll::Pending` de Tokio si la cola está vacía.
            // Registra un "Waker" (callback) en el sistema. El thread NO se bloquea, 
            // simplemente busca otro Worker listo en la Readiness Queue.
            while let Ok(numero_slot) = rx_clon.recv().await {
                
                let max_reintentos = 10;
                let mut bloque_encontrado = false;

                // Loop de reintentos
                for intento in 1..=max_reintentos {
                    
                    // Armamos el pedido usando JSON-RPC 2.0 (Remote Procedure Call)
                    // Básicamente es llamar una función "getBlock" en otro servidor.
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

                    // Hacemos el pedido POST HTTP.
                    // .await -> Esta operación cruza la red a velocidades humanas. 
                    // El future devuelve "Pending", el OS Kernel activa el epoll para
                    // esperar la respuesta, y este Hilo se libera momentáneamente.
                    match http_clon.post(URL_SOLANA_HTTP).json(&request_body).send().await {
                        Ok(resp) => {
                            // Llegaron los "Headers" HTTP. Ahora `json().await` espera que
                            // llegue todo el resto del "Body" gigantesco de megabytes, y 
                            // luego usa Serde para 'parsearlo' (traducir bytes crudos
                            // de Base-58/Borsh a objetos estructurados de Rust).
                            if let Ok(block_data) = resp.json::<Value>().await {
                                
                                // ¿El servidor respondió con error (el bloque aún no existe)?
                                if block_data.get("error").is_some() {
                                    // Sistema de "BACKOFF LINEAL". 
                                    // Si hay falla, la espera aumenta antes de reintentar 
                                    // (500ms, 1000ms, 1500ms...) para no explotar la API.
                                    let demora_ms = 500 * intento; 
                                    println!("  [Worker #{}] Slot {} demorado (intento {}/{}). Esperando {}ms...", id_worker, numero_slot, intento, max_reintentos, demora_ms);
                                    
                                    // `tokio::time::sleep` asincrónico.
                                    // Le avisa al timer del kernel: "Despertá este Future 
                                    // con un Waker dentro de X ms". Otros workers siguen ruteando.
                                    tokio::time::sleep(Duration::from_millis(demora_ms)).await;
                                    continue; 
                                }

                                // Si el bloque está, usamos extractores `if let Som()` para 
                                // evitar hacer panic si los datos estuvieran corruptos.
                                if let Some(resultado_bloque) = block_data.get("result") {
                                    // Pasamos a procesado sincrónico puro
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

    // ==============================================================================
    // 🔊 EL PRODUCTOR PRINCIPAL (WEBSOCKET)
    // El hilo principal o 'main thread' se dedica pura y exclusivamente a esto.
    // ==============================================================================
    loop {
        println!("Intentando conectar al WebSocket de Chainstack...");

        let url = Url::parse(URL_SOLANA_WS)?;
        
        // Arrancamos con un "Handshake" TCP, y luego un Upgrade a WSS (WebSocket Secure/TLS)
        let (ws_stream, _) = match connect_async(url).await {
            Ok(s) => s,
            Err(e) => {
                println!("Error WS, reintentando en 3 segundos... ({})", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        println!("Conectado! Enviando suscripcion de SLOTS...\n");
        
        // Separamos el tubo bidireccional en dos (Ownership borrow rules). 
        // Si no lo dividimos, Rust no dejaría cruzar lectura con escritura simultánea.
        let (mut write, mut read) = ws_stream.split();

        // Le avisamos al nodo de cadena: 
        // "Quiero que cada vez que pase un slot, me mandes un mini paquete 'Ding!'"
        let mensaje_suscripcion = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "slotSubscribe"
        });

        // Este mensaje va envuelto con un 'Frame' de websocket + KeepAlive interno.
        if let Err(e) = write.send(Message::Text(mensaje_suscripcion.to_string())).await {
            println!("Error enviando suscripcion: {}", e);
            continue;
        }

        // 🚨 Loop de escaneo ultrarrápido
        loop {
            // El thread asincrónico pasa dormido en la Readiness Queue acá la mayor
            // parte del tiempo. Despierta mediante epoll cuando un slot nuevo es cantado.
            if let Some(respuesta) = read.next().await {
                match respuesta {
                    Ok(mensaje) => {
                        if let Message::Text(texto) = mensaje {
                            let json_parseado: Result<Value, _> = serde_json::from_str(&texto);
                            if let Ok(datos) = json_parseado {
                                if let Some(slot_obj) = datos.pointer("/params/result/slot") {
                                    if let Some(numero_slot) = slot_obj.as_u64() {
                                        
                                        println!("🔔 [WS Rápido] Nuevo Slot detectado: {}", numero_slot);
                                        
                                        // Empujar slot al 'buffer' (Canal MPMC). 
                                        // Acá ocurre la entrega formal al ejército de Workers.
                                        if let Err(e) = tx_slots.send(numero_slot).await {
                                            println!("Error tirando slot al canal: {}", e);
                                        }
                                        
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Router mató la conexión? Server cayó?
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
// Función puramente sincrónica porque no hay latencia de Red; la data ya está en RAM.
// ==============================================================================
fn procesar_y_guardar_bloque(numero_slot: u64, resultado_bloque: &Value, id_worker: usize) {
    
    // Parseo de Unix Epoch (segundos desde 1 de Enero de 1970) a fecha humana.
    let unix_time = resultado_bloque["blockTime"].as_i64().unwrap_or(0);
    let datetime: DateTime<Utc> = Utc.timestamp_opt(unix_time, 0).unwrap();
    
    // Extracción de Cantidad de arreglos usando closure sintaxis | |
    let tx_count = resultado_bloque["transactions"].as_array().map(|arr| arr.len()).unwrap_or(0);
    let mut swaps_encontrados = 0;

    // Buscamos si la base de datos "transactions" está dentro del Json original
    if let Some(transacciones) = resultado_bloque["transactions"].as_array() {
        for tx in transacciones {
            
            // ARCHITECTURE HACK DE SOLANA (COMPRESIÓN):
            // En vez de escribir largas direcciones de cuentas (Pubkeys) en cada instrucción,
            // Solana indexa como en el preámbulo de un guión de teatro al "elenco" necesario 
            // de toda esta transacción. El actor 0 es usuario, el actor 1 es Jupiter, etc.
            let account_keys = &tx["transaction"]["message"]["accountKeys"];

            // Luego en las escenas ('instrucciones'), la transacción sólo se refiere a sus 
            // participantes con punteros `programIdIndex`: "El programa [Índice 1] accionó".
            if let Some(instrucciones) = tx["transaction"]["message"]["instructions"].as_array() {
                for instruccion in instrucciones {
                    
                    if let Some(idx) = instruccion["programIdIndex"].as_u64() {
                        
                        // Rescatamos del ELENCO a este participante numérico.
                        let program_id = account_keys[idx as usize].as_str().unwrap_or("");
                        
                        // ¿El participante que ejecutó se llama igual a Júpiter Agregator?
                        if program_id == JUPITER_ID {
                            swaps_encontrados += 1;
                        }
                    }
                }
            }
        }
    }

    // Imprimir resultado en consola
    let linea_resultado = format!("[Worker #{}] Slot: {} | {} | Total Txs: {} | Swaps Jupiter: {}\n",
        id_worker,
        numero_slot,
        datetime.format("%Y-%m-%d %H:%M:%S"),
        tx_count,
        swaps_encontrados
    );

    print!("  ✅ ¡ÉXITO! | {}", linea_resultado);

    // Escribir a un archivo (Será reemplazado pr SQLite próximamente)
    let mut archivo = OpenOptions::new()
        .create(true)
        .append(true)
        .open("jupiter_swaps.txt")
        .expect("No se pudo abrir el archivo para guardar");

    if let Err(e) = write!(archivo, "{}", linea_resultado) {
        println!("Error escribiendo en el archivo: {}", e);
    }
}
