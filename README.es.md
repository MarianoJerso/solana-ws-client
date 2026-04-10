# solana-ws-client

[Read in English](README.md)

`solana-ws-client` es un nodo de ingestión de datos asincrónico para la blockchain de Solana. Emplea una arquitectura orientada a eventos para indexar la actividad on-chain en tiempo real, enfocándose actualmente en el volumen de enrutamiento automatizado.

## Arquitectura

El sistema desacopla la transmisión de notificaciones de la obtención de datos pesados para mitigar bloqueos de línea ("head-of-line blocking"):

1. **Capa de Notificaciones**: Una conexión WebSocket se suscribe a las notificaciones de slots de Solana (`slotSubscribe`) para mantener un flujo de eventos de baja latencia.
2. **Canal MPMC**: Un canal limitado de múltiples productores y múltiples consumidores (MPMC) actúa como búfer para las notificaciones de slots entrantes, proporcionando control de contrapresión (backpressure).
3. **Workers Asincrónicos**: Un pool configurable de workers basados en Tokio consume del canal, ejecutando peticiones JSON-RPC POST para obtener los datos completos de los bloques.
4. **Persistencia**: Las métricas de los bloques extraídos y los agregados de transacciones se escriben de manera asincrónica en una base de datos local SQLite utilizando `sqlx`.

## Detalles Técnicos

- **Runtime**: Utiliza `tokio` para operaciones de I/O no bloqueantes y planificación de tareas (scheduling).
- **Concurrencia**: El estado se comparte entre los workers mediante `Arc` (pools de conexión de reqwest, pools de base de datos de sqlx) para evitar duplicación en memoria.
- **Confiabilidad**: Las peticiones de red implementan una estrategia de retroceso lineal (linear backoff) para manejar límites de ratio y retrasos en la propagación de bloques.
- **Análisis Sintáctico (Parsing)**: La inspección directa del arreglo `accountKeys` y los punteros de instrucción (`programIdIndex`) minimiza la fragmentación de memoria al identificar contratos inteligentes específicos (ej. Jupiter Aggregator).

## Uso

### Dependencias

- Rust (Edición 2024 o superior)

### Compilar y Ejecutar

```bash
git clone https://github.com/MarianoJerso/solana-ws-client.git
cd solana-ws-client
cargo run --release
```

Al inicializarse, la aplicación creará un archivo de base de datos SQLite `swaps.db` en el directorio raíz y comenzará a indexar los datos de los bloques automáticamente.
