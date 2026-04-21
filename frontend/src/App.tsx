// Phase 8 placeholder — the memory webpage (markdown editor + row browser)
// has not been built yet. See ~/.claude/plans/memory-system-rebuild.md,
// Phase 8, for the planned replacement.
//
// For now this renders a simple status page so the Vite build still
// produces an index.html that the backend can embed via rust_embed.

export default function App() {
  return (
    <div
      style={{
        fontFamily: 'system-ui, -apple-system, sans-serif',
        color: '#e5e7eb',
        background: '#0a0a0a',
        minHeight: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '2rem',
      }}
    >
      <div style={{ maxWidth: 640, lineHeight: 1.6 }}>
        <h1 style={{ fontSize: '1.5rem', fontWeight: 700, marginBottom: '0.5rem' }}>
          linggen-memory
        </h1>
        <p style={{ color: '#9ca3af', marginBottom: '1.5rem' }}>
          Memory service — facts, activity, semantic retrieval.
        </p>
        <p>
          The browser UI is being rebuilt for the memory-skill use case. Until
          Phase 8 ships, interact via the CLI:
        </p>
        <pre
          style={{
            background: '#1a1a1a',
            border: '1px solid #333',
            padding: '0.75rem 1rem',
            borderRadius: 6,
            fontSize: '0.875rem',
            marginTop: '1rem',
          }}
        >
          linggen-memory add "fact text" --context music/piano{'\n'}
          linggen-memory search "jazz practice"{'\n'}
          linggen-memory list --type activity
        </pre>
      </div>
    </div>
  )
}
