use chrono::{DateTime, Utc, TimeZone}; // Para manejar fechas legibles
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. VOLVEMOS AL SERVIDOR PÚBLICO GRATUITO DE SOLANA
    let url_solana = "wss://solana-mainnet.core.chainstack.com/4f6a60821bf93dc2d08750216894e530";
    let url = Url::parse(url_solana)?;

    println!("Intentando conectar a {}...", url_solana);
    //nos conectamos con el servidor de solana usando su api publica y creamos una conexion wss
    let (ws_stream, _) = connect_async(url).await?;
    println!("¡Conectado con éxito al túnel público de Solana!\n");
    //consumimos la estructura wss y para crear dos nuevas estructuras wss que separan los trait sink y stream, esto es para poder enviar mensajes al servidor mientras recimos mensajes de este de manera asincronica
    let (mut write, mut read) = ws_stream.split();

     // 2. EL MENSAJE ORIGINAL DEL RETO (blockSubscribe)
    // Ahora le pedimos el bloque entero, pero sin el masacote de datos completos
    // (solo los IDs de las transacciones, suficiente para contarlas).
    let mensaje_suscripcion = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "blockSubscribe",
        "params": [
            "all", 
            {
                "encoding": "json",
                // "signatures" nos da un array con un ID por transacción. 
                // Contar cuántos elementos tiene ese array será nuestro total.
                "transactionDetails": "signatures", 
                "showRewards": false,
                "maxSupportedTransactionVersion": 0
            }
        ]
    });
    //enviamos el mensaje usando la estructura con el trait sink y convertimos el mensaje en el tipo que entiende el servidor de Solana, en este caso seria un string
    write.send(Message::Text(mensaje_suscripcion.to_string())).await?;
     println!("Suscripción a 'Bloques Completos' enviada.\n");
     println!("Esperando recibir la Matrix... (Puede tardar 1-2 segundos)\n");

    // 3. EL BUCLE INFINITO (LOOP)
    // "loop" en Rust significa: haz esto por toda la eternidad hasta que yo cierre el programa (Ctrl+C).
    // Aquí es donde brilla el ".await" de Tokio. Mientras no lleguen mensajes,
    // el programa se queda "dormido" sin gastar CPU. Cuando llega un mensaje, se despierta,
    // lo imprime y vuelve a dormirse.
    loop {
        if let Some(respuesta) = read.next().await {
            match respuesta {
                Ok(mensaje) => {
                    // Si el mensaje es texto, lo imprimimos por pantalla
                        if let Message::Text(texto) = mensaje {
                        let json_parseado: Result<serde_json::Value, _> = serde_json::from_str(&texto);
                        
                        if let Ok(datos) = json_parseado {
                            // IMPORTANTE:
                            // El primer mensaje que envía siempre es una confirmación boba de que te suscribiste.
                            // Solo procesamos los datos si la "carpeta" donde vienen los bloques existe 
                            // (Esa carpeta ahora se llama "value", y dentro está el "block").
                            if let Some(bloque) = datos.pointer("/params/result/value/block") {
                                
                                // Sacamos el Slot
                                let slot = datos["params"]["result"]["context"]["slot"].as_u64().unwrap_or(0);
                                
                                // Sacamos la cantidad de transacciones (Pidiendo la longitud del arreglo "signatures")
                                let tx_count = bloque["signatures"].as_array().map(|arr| arr.len()).unwrap_or(0);
                                
                                // Sacamos el Tiempo (Solana lo manda en "blockTime", que son Segundos Unix de 1970)
                                let unix_time = bloque["blockTime"].as_i64().unwrap_or(0);
                                
                                // Magia de la librería Chrono: Convierte los segundos en Fecha y Hora legible (UTC)
                                let datetime: DateTime<Utc> = Utc.timestamp_opt(unix_time, 0).unwrap();
                                // ¡BINGO! El objetivo final propuesto:
                                println!("📦 BLOQUE RECIBIDO | Slot: {} | Tx's: {} | Fecha/Hora: {}", 
                                        slot, 
                                        tx_count, 
                                        datetime.format("%Y-%m-%d %H:%M:%S")
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("Túnel colapsado: {}", e);
                    break;
                }
            }
        }
    }

        Ok(())
}


