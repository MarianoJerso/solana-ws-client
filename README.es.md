# 🦀 Nodo de Ingestión de Datos de Solana

![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)
![Solana](https://img.shields.io/badge/Solana-14F195?style=for-the-badge&logo=solana&logoColor=white)
![SQLite](https://img.shields.io/badge/SQLite-07405E?style=for-the-badge&logo=sqlite&logoColor=white)

*[🇬🇧 Read in English](README.md)*

Un nodo de ingestión de datos asincrónico y de alto rendimiento para la blockchain de Solana. Construido íntegramente en Rust, este sistema demuestra conceptos avanzados de sistemas distribuidos, concurrencia segura en memoria y arquitectura orientada a eventos para indexar la actividad de exchanges descentralizados (DEX) en tiempo real.

---

## 🏗️ Diseño de Arquitectura (Event-Driven Fetching)

Este proyecto evita los típicos cuellos de botella ("Head-of-Line blocking") separando la capa de notificaciones de la capa pesada de descarga de datos:

1. **Capa Ligera de Notificaciones:** Una conexión WebSocket se suscribe a los eventos de nuevos slots en la red. Escucha notificaciones minúsculas ("Ding! Nuevo slot X") y las empuja inmediatamente a un búfer interno.
2. **Desacoplamiento (Canal MPMC):** Un canal MPMC (Multi-Producer Multi-Consumer) con límite dinámico actúa como red de seguridad y amortiguador entre el WebSocket ultrarrápido y las descargas HTTP más lentas.
3. **Capa de Procesamiento Pesado (Workers Asincrónicos):** Un ejército de 10 trabajadores concurrentes consume el canal MPMC constantemente. Ejecutan peticiones HTTP POST (JSON-RPC) para descargar el bloque de datos completo (de varios megabytes) y parsean miles de transacciones por bloque en paralelo.

## 🧠 Conceptos Clave de Ingeniería de Sistemas

Este repositorio sirve como portafolio demostrativo de programación a bajo nivel:

*   **I/O No Bloqueante (`tokio` & epoll):** Se evita por completo el bloqueo de los hilos físicos del Sistema Operativo. Los Workers ceden el control activamente al runtime de `tokio` (Ready/Pending Queue) mientras esperan respuestas de red TCP/TLS o de lectura/escritura en Disco.
*   **Concurrencia con Seguridad de Memoria (`Arc<T>`):** Utilización de Contadores de Referencia Atómicos (Atomic Reference Counters) para compartir el pool de conexiones de `reqwest` y el pool de base de datos de `sqlx` entre todos los Workers simultáneamente sin usar costosos bloqueos mutuos o duplicar RAM.
*   **Tolerancia a Fallos (Backoff Retry):** El sistema utiliza un algoritmo de retroceso lineal (500ms * intento) para reponerse suavemente de caídas temporales de red o propagaciones tardías de bloques en Solana, evitando el crasheo del programa.
*   **Parsing Estructural de Protocolos:** Navegación directa sobre jerarquías complejas tipo JSON para resolver el sistema de compresión de Solana (utilizando punteros de `programIdIndex` hacia sub-listas de `accountKeys`) para aislar operaciones específicas de contratos inteligentes.
*   **Persistencia Asincrónica Relacional:** Almacenamiento seguro en disco utilizando una Base de Datos SQLite, insertando la información estructurada a través de directivas asincrónicas de la librería `sqlx` chequeadas en tiempo de compilación.

## 🚀 Inicio Rápido

### Prerrequisitos
- [Rust](https://www.rust-lang.org/) (Edición 2024)
- Conexión a internet para compilar las dependencias (`crates`).

### Instalación y Ejecución
```bash
# Clonar el proyecto
git clone https://github.com/MarianoJerso/solana-ws-client.git
cd solana-ws-client

# Iniciar el nodo
cargo run
```

Al iniciarse, el sistema creará automáticamente el archivo relacional `swaps.db` y comenzará a ingestar y analizar bloques de la blockchain en vivo.

## 📊 Extracción de Datos (Jupiter Routing)

La base de datos actual está configurada para recopilar y cuantificar el volumen transaccional de **Jupiter Aggregator**, el DEX principal de Solana. En cada bloque se guarda:
- `slot_number`: El bloque exacto en la cadena.
- `unix_timestamp`: La fecha y hora plana universal (lista para matemáticas de series temporales).
- `tx_count`: Conteo absoluto de transacciones de toda la red en ese bloque.
- `jupiter_swaps`: Veces exactas que un usuario interactuó con el smart contract de Jupiter.

*Estos datos representan la capa de infraestructura fundacional requerida para construir Detección de MEV, análisis cuantitativos del mercado o motores de trading algorítmico.*
