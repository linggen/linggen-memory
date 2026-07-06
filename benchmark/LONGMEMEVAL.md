# LongMemEval-S — ling-mem

[LongMemEval](https://arxiv.org/abs/2410.10813) (Wu et al., ICLR 2025) measures long-term memory across multi-session chat. The S variant has 500 questions, ~48 sessions per question, ~115K tokens of conversation.

## Setup

- **Dataset**: [`xiaowu0162/longmemeval-cleaned`](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned), split `longmemeval_s_cleaned`
- **Metric**: `recall_any@K` — does *any* gold session appear in top-K retrieved?
- **Granularity**: one fact per turn (LongMemEval `GRANULARITY=turn`; sub-chunked under the 512-token embedding cap)
- **Mode**: `ling-mem search` default (hybrid: cosine + IDF-weighted keyword boost)
- **No LLM in the loop** — pure retrieval eval, no answer generation, no judge

## Reproducing

See [`README.md`](README.md). Headline:

```bash
python benchmark/run_longmemeval.py \
  --dataset benchmark/data/longmemeval_s.jsonl \
  --out benchmark/results/longmemeval_s.json
```

## Results

> Pending. Replace this section with the runner's markdown output after the first full run.
>
> Format:
>
> | ling-mem version | embedding model | R@5 | R@10 | R@20 | MRR | NDCG@10 |
> |---|---|---|---|---|---|---|
> | v0.5.1 | (name) | TBD | TBD | TBD | TBD | TBD |

### By question type

| Type | n | R@5 | R@10 |
|---|---|---|---|
| (pending) | — | — | — |

## Comparison

> Reference numbers from prior work on the same benchmark (LongMemEval-S, R@5):
>
> | System | R@5 | Notes |
> |---|---|---|
> | agentmemory (BM25 + Vector) | 95.2% | `all-MiniLM-L6-v2`, no API key |
> | agentmemory (BM25 only) | 86.2% | fallback |
> | MemPalace (vector only) | ~96.6% | larger embedding model |
> | ling-mem | TBD | (this row) |
>
> Letta/MemGPT (83.2%) and mem0 (68.5%) are published on **LoCoMo**, not LongMemEval — different dataset, not directly comparable. See [`COMPARISON.md`](COMPARISON.md) for the apples-to-oranges chart.

## Notes on methodology

- **Per-question isolation**: every question gets a fresh `LINGGEN_DATA_DIR` so retrievals can't leak across questions.
- **Tag-based session recovery**: each indexed session is tagged `session/<id>`; the runner reads the tag back from search results to map facts → sessions.
- **Single-mode benchmark**: this number reflects `ling-mem search` as users actually call it. Separating into BM25-only / vector-only numbers requires a `--mode` flag on `search` that doesn't exist today.
- **Run time**: ~40 minutes on a single M1/M2 machine for the full 500-question run.
