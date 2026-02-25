# Linggen Memory Framework Architecture

## High-Level Architecture

The system consists of a Rust backend service (`ling-mem`) that handles data ingestion, embedding generation, and vector storage (LanceDB). A React frontend provides the web dashboard, and external tools (VS Code, Cursor, Claude Code) connect via REST API or MCP.

```mermaid
graph TD
    User[User] -->|Interacts| UI["Web Dashboard (React)"]
    User -->|Uses| IDE[VS Code Extension]

    subgraph "Local Machine"
        UI -->|REST API| API[Axum API Server]
        IDE -->|REST API| API

        subgraph "Rust Backend Service"
            API --> Controller[Logic Controller]

            Controller -->|Ingest| Ingest[Ingestion Engine]
            Ingest -->|Watch| FS[Local Filesystem]
            Ingest -->|Clone| Git[Git Repos]
            Ingest -->|Crawl| Web[Doc Websites]

            Controller -->|Embed| Model["Embedding Model (Candle)"]

            Controller -->|Store/Query| VectorDB
            VectorDB -->|Disk Storage| Storage[Local Disk / S3]
        end
    end
```

## Components

### 1. API Server (Axum)

- Exposes REST endpoints for `query`, `ingest`, `status`.
- Handles WebSocket connections for real-time indexing updates.

### 2. Ingestion Engine

- **File Watcher**: Uses `notify` crate to watch for file changes.
- **Git Indexer**: Uses `git2` to read repository history.
- **Web Crawler**: Uses `spider` or `reqwest` to fetch documentation.

### 3. Vector Store (LanceDB)

- Embedded, serverless vector database.
- Stores embeddings and metadata on the local disk.
- Supports hybrid search (Vector + Full Text).

### 4. Embedding Model

- Runs locally using `candle` (HuggingFace Rust ecosystem).
- Default model: `all-MiniLM-L6-v2` (fast, lightweight).
