# Linggen Frontend

React + TypeScript + Vite web UI for Linggen.

## Development

```bash
cd frontend
npm install
npm run dev
```

The UI will be available at `http://localhost:5173`. It expects the Linggen backend to be running at `http://localhost:8787`.

## Build

```bash
npm run build
```

The build output in `dist/` is embedded into the `linggen-server` binary at compile time.
