# benchmark/

Reproducible quality numbers for `ling-mem`.

## What's in here

- `run_longmemeval.py` — runner that drives the `ling-mem` CLI through the LongMemEval-S benchmark and emits recall/MRR/NDCG.
- `LONGMEMEVAL.md` — methodology and the latest results.
- `data/` — gitignored. Dataset goes here, fetched on demand (see below).
- `results/` — gitignored. Per-run JSON output.

## Run the benchmark

### 1. Fetch the dataset (~250 MB, one-time)

```bash
pip install datasets
python -c "
from datasets import load_dataset
ds = load_dataset('xiaowu0162/longmemeval-cleaned', split='longmemeval_s_cleaned')
ds.to_json('benchmark/data/longmemeval_s.jsonl', orient='records', lines=True)
"
```

### 2. Make sure `ling-mem` is on PATH

```bash
which ling-mem && ling-mem --version
```

Install with the standard wrapper if needed:

```bash
curl -fsSL https://linggen.dev/install-ling-mem.sh | bash
```

### 3. Run

```bash
# Smoke test (10 questions, ~1 min)
python benchmark/run_longmemeval.py \
  --dataset benchmark/data/longmemeval_s.jsonl \
  --out benchmark/results/smoke.json \
  --limit 10

# Full run (500 questions, ~40 min)
python benchmark/run_longmemeval.py \
  --dataset benchmark/data/longmemeval_s.jsonl \
  --out benchmark/results/longmemeval_s_v$(ling-mem --version | awk '{print $2}').json
```

The runner prints a markdown summary on stdout and writes the full per-question result to `--out`.

## What the runner does

Per question, in isolation:

1. Spin up a fresh `LINGGEN_DATA_DIR` (so cross-question contamination is impossible).
2. Index each of the ~48 haystack sessions as one fact, tagged `session/<id>`.
3. Query with the question text via `ling-mem search`.
4. Extract retrieved `session_id`s from the result rows' tags.
5. Compute `recall_any@{5,10,20}`, MRR, NDCG@10 against the gold `answer_session_ids`.
6. Tear down the data dir.

Aggregates per question type and overall.

## Caveats

- The runner measures `ling-mem`'s **default search mode** (hybrid: vector + metadata). To benchmark BM25-only or vector-only separately, ling-mem needs a `--mode` flag on `search` (not implemented yet).
- Numbers are tied to the embedding model `ling-mem` ships with at the version under test. Always record the version in the filename and in `LONGMEMEVAL.md`.
- Granularity here is **session-as-fact** (one row per haystack session, ~5 KB each). This matches the apples-to-apples comparison with agentmemory and MemPalace. ling-mem's actual usage pattern is shorter atomic facts; if you want a "natural-granularity" number, build a separate Option-B runner that adds each turn as its own fact.
