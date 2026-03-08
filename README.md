# Cliente WebSocket Público de Solana en Rust 🦀

Un proyecto inicial para aprender asincronía y redes en Rust. Este programa se conecta al endpoint público de la blockchain de Solana y formatea en la consola cada nuevo "Slot" (latido de tiempo de la blockchain), procesando la comunicación JSON-RPC en tiempo real.

## 🛠️ Tecnologías Utilizadas

- **Rust** (Lenguaje principal)
- **Tokio:** Para el manejo de programación asíncrona.
- **Tungstenite (`tokio-tungstenite`):** Para la conexión continua vía WebSockets.
- **Serde (`serde_json`):** Para la deserialización y lectura del formato JSON.

## 🚀 Instrucciones de Uso

Para probar este cliente en tu máquina local:

1. Asegúrate de tener Rust y Cargo instalados.
2. Clona este repositorio y navega hasta la carpeta del proyecto.
3. Ejecuta el siguiente comando en tu terminal:

```bash
cargo run
