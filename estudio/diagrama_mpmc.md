# Diagrama MPMC — Nodo de Solana

## Arquitectura completa del flujo de datos

```mermaid
flowchart TD
    CHAIN["🌐 Chainstack / Solana Mainnet\nwss://solana-mainnet.core.chainstack.com"]

    subgraph PRODUCTOR["🔊 PRODUCTOR — Loop Principal (1 solo hilo)"]
        WS["WebSocket\ntokio_tungstenite\nread.next().await"]
        PARSE["Parsear JSON\nserde_json\nExtraer numero_slot (u64)"]
        SEND["tx_slots.send(numero_slot).await"]
    end

    subgraph CANAL["📦 CANAL MPMC — async_channel::bounded(500)"]
        direction LR
        S1["slot 300001"]
        S2["slot 300002"]
        S3["slot 300003"]
        S4["...hasta 500"]
    end

    subgraph WORKERS["⚙️ CONSUMIDORES — 10 Workers en paralelo (10 hilos de Tokio)"]
        W1["Worker #1\nrx_clon.recv().await\n→ POST HTTP\n→ getBlock(slot)"]
        W2["Worker #2\nrx_clon.recv().await\n→ POST HTTP\n→ getBlock(slot)"]
        W3["Worker #3\nrx_clon.recv().await\n→ POST HTTP\n→ getBlock(slot)"]
        WN["Workers #4–#10\n(idénticos)"]
    end

    ARC["🔗 Arc-Client (compartido)\nreqwest::Client\nConnection Pool HTTP/TLS"]

    subgraph PROCESO["📊 procesar_y_guardar_bloque()"]
        ITER["Iterar transacciones"]
        FILT["Filtrar por Jupiter ID\nJUP6Lkb..."]
        OUT["Escribir resultado\njupiter_swaps.txt"]
    end

    CHAIN -->|"Ping cada ~400ms\n'nuevo slot listo'"| WS
    WS --> PARSE
    PARSE --> SEND
    SEND -->|"Entra al buffer"| CANAL

    CANAL -->|"recv() — solo 1 worker lo agarra"| W1
    CANAL -->|"recv() — solo 1 worker lo agarra"| W2
    CANAL -->|"recv() — solo 1 worker lo agarra"| W3
    CANAL -->|"recv()"| WN

    ARC -->|"http_clon.post()"| W1
    ARC -->|"http_clon.post()"| W2
    ARC -->|"http_clon.post()"| W3
    ARC -->|"http_clon.post()"| WN

    W1 -->|"bloque completo"| PROCESO
    W2 -->|"bloque completo"| PROCESO
    W3 -->|"bloque completo"| PROCESO
    WN -->|"bloque completo"| PROCESO

    PROCESO --> ITER --> FILT --> OUT
```

---

## Detalle del ciclo de vida de un slot

```mermaid
sequenceDiagram
    participant SOL as Solana Network
    participant WS as WebSocket Loop
    participant CH as Canal MPMC (buffer)
    participant W3 as Worker #3
    participant API as Chainstack HTTP API
    participant FILE as jupiter_swaps.txt

    SOL->>WS: "nuevo slot: 300042"
    WS->>WS: parsear JSON con serde_json
    WS->>CH: tx_slots.send(300042)
    Note over CH: 300042 entra a la cola

    Note over W3: estaba libre esperando recv()
    CH->>W3: 300042 (solo Worker #3 lo recibe)

    W3->>API: POST getBlock(300042) via reqwest + Arc<Client>
    Note over W3: .await — cede el hilo mientras espera
    API-->>W3: { transactions: [...] }

    W3->>W3: procesar_y_guardar_bloque()
    W3->>W3: filtrar instrucciones de Jupiter
    W3->>FILE: escribir resultado
    Note over W3: vuelve a recv() esperando el próximo slot
```

---

## Lo que el Arc hace en memoria

```
                  RAM
                   │
          ┌────────▼────────┐
          │  reqwest::Client │
          │  ┌─────────────┐ │
          │  │ TCP Pool    │ │
          │  │ TLS Config  │ │
          │  │ dns cache   │ │
          │  └─────────────┘ │
          │  strong_count: 11│  ← 1 original + 10 clones
          └─────────────────┘
               ↑  ↑  ↑
         W1    W2  W3  ... W10
       (cada uno tiene un puntero Arc,
        no una copia del Client)
```
