#!/usr/bin/env python3
"""
LongMemEval-S runner for ling-mem.

Architecture (v0.3 — isolated bench daemon):
  - The whole run uses ONE throwaway data dir under $TMPDIR. The user's
    real store (~/.linggen) and their daemon on port 9888 are never
    touched — no pollution of their recall hook, no backup/restore needed.
  - A dedicated `ling-mem serve` is started on an alternate port against
    that temp dir. All `ling-mem --data-dir <tmp>` calls auto-discover it
    (≈5× faster than direct-store mode). We own its PID and kill it on
    exit, so it can never strand on the default port.
  - Per-question isolation via `--context q_<qid>`: every row carries the
    question id; `search --context q_<qid>` only sees that question's
    haystack. No inter-question contamination.
  - Session recovery via `--tag session/<sid>`, read back off each hit.

Usage:
  python run_longmemeval.py --dataset data/longmemeval_s.jsonl --out results/full.json
  python run_longmemeval.py --dataset data/longmemeval_s.jsonl --limit 10
  python run_longmemeval.py --dataset data/longmemeval_s.jsonl --shuffle --seed 0 --limit 30
"""
import argparse
import datetime
import json
import math
import os
import random
import uuid
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from collections import defaultdict
from pathlib import Path

K_VALUES = (5, 10, 20)
BENCH_PORT = 19888


def run_ling(cmd, data_dir, *, input_text=None, timeout=300):
    env = {**os.environ, "LING_MEM_NO_TELEMETRY": "1",
           "LINGGEN_DATA_DIR": str(data_dir)}
    p = subprocess.run(["ling-mem", "--data-dir", str(data_dir), *cmd],
                        env=env, input=input_text,
                        capture_output=True, text=True, timeout=timeout)
    if p.returncode != 0:
        raise RuntimeError(f"ling-mem {cmd[0]} failed: {p.stderr.strip()[:200]}")
    return p.stdout


def daemon_healthy(port):
    try:
        with urllib.request.urlopen(f"http://localhost:{port}/api/health",
                                    timeout=3) as r:
            return b'"ok":true' in r.read()
    except Exception:
        return False


def start_bench_daemon(data_dir):
    env = {**os.environ, "LING_MEM_NO_TELEMETRY": "1",
           "LINGGEN_DATA_DIR": str(data_dir)}
    proc = subprocess.Popen(
        ["ling-mem", "serve", "--port", str(BENCH_PORT)],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    for _ in range(30):
        if daemon_healthy(BENCH_PORT):
            return proc
        time.sleep(0.5)
    proc.terminate()
    raise RuntimeError(f"bench daemon failed to bind on :{BENCH_PORT}")


# Turn-granularity (LongMemEval GRANULARITY=turn). ling-mem caps embedding
# input at 512 tokens; ~95% of haystack *sessions* exceed that, so a
# session-as-blob index measures the cap, not retrieval. Index one fact per
# turn; sub-chunk any turn still over budget. ~4 chars/token → 1600 chars
# (~400 tok) keeps every chunk safely under the cap; 200-char overlap so an
# answer span isn't split exactly on a boundary.
CHUNK_CHARS = 1600
CHUNK_OVERLAP = 200


def turn_chunks(turns):
    """Yield text chunks: one per turn, sub-windowed if a turn is too long."""
    for t in turns:
        text = f"{t['role']}: {t['content']}".strip()
        if not text:
            continue
        if len(text) <= CHUNK_CHARS:
            yield text
            continue
        step = CHUNK_CHARS - CHUNK_OVERLAP
        for i in range(0, len(text), step):
            piece = text[i:i + CHUNK_CHARS]
            if piece:
                yield piece


def index(q, data_dir):
    """One `add --stdin` call per question: every turn-chunk is a fact row
    tagged session/<sid>, scoped to this question's context. Batching via
    NDJSON stdin collapses ~hundreds of subprocess spawns into one."""
    ctx = f"q_{q['question_id']}"
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    rows = []
    for sid, turns in zip(q["haystack_session_ids"], q["haystack_sessions"]):
        for chunk in turn_chunks(turns):
            # `add --stdin` deserializes each line into a full Fact:
            # id + created_at are required (the positional `add` generates
            # them; the bulk path does not).
            rows.append(json.dumps({
                "id": str(uuid.uuid4()),
                "content": chunk,
                "type": "fact",
                "from": "derived",
                "contexts": [ctx],
                "tags": [f"session/{sid}"],
                "created_at": now,
            }))
    if not rows:
        return
    run_ling(["add", "--stdin", "--quiet"], data_dir,
             input_text="\n".join(rows) + "\n")


def query(q, data_dir, k=20):
    out = run_ling(["search", q["question"],
                    "--context", f"q_{q['question_id']}",
                    "--limit", str(k), "--format", "json", "--quiet"],
                   data_dir)
    sids = []
    for line in out.strip().splitlines():
        if not line:
            continue
        row = json.loads(line)
        for tag in row.get("tags", []):
            if tag.startswith("session/"):
                sids.append(tag[len("session/"):])
                break
    return sids


def recall_at_k(retrieved, gold, k):
    return 1.0 if set(retrieved[:k]) & set(gold) else 0.0


def mrr(retrieved, gold):
    gs = set(gold)
    for i, r in enumerate(retrieved):
        if r in gs:
            return 1.0 / (i + 1)
    return 0.0


def ndcg(retrieved, gold, k=10):
    gs = set(gold)
    def dcg(rels): return sum(r / math.log2(i + 2) for i, r in enumerate(rels[:k]))
    rels = [1.0 if r in gs else 0.0 for r in retrieved]
    idcg = dcg([1.0] * min(k, len(gs)))
    return dcg(rels) / idcg if idcg else 0.0


def score(retrieved, gold):
    return {**{f"r@{k}": recall_at_k(retrieved, gold, k) for k in K_VALUES},
            "mrr": mrr(retrieved, gold), "ndcg@10": ndcg(retrieved, gold)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--shuffle", action="store_true")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    questions = [json.loads(l) for l in open(args.dataset) if l.strip()]
    if args.shuffle:
        random.Random(args.seed).shuffle(questions)
    if args.limit:
        questions = questions[:args.limit]

    tmp = Path(tempfile.mkdtemp(prefix="lme-ling-"))
    print(f"# bench data dir: {tmp}", file=sys.stderr)
    print(f"# starting bench daemon on :{BENCH_PORT} ...", file=sys.stderr)
    daemon = start_bench_daemon(tmp)
    print(f"# bench daemon up (pid {daemon.pid})", file=sys.stderr)

    rows, by_type = [], defaultdict(list)
    try:
        for i, q in enumerate(questions):
            qid, qtype = q["question_id"], q["question_type"]
            t0 = time.time()
            try:
                index(q, tmp)
                retrieved = query(q, tmp, k=max(K_VALUES))
                s = score(retrieved, q["answer_session_ids"])
            except Exception as e:
                print(f"  [{qid}] FAILED: {e}", file=sys.stderr)
                retrieved = []
                s = {f"r@{k}": 0.0 for k in K_VALUES} | {"mrr": 0.0, "ndcg@10": 0.0}
            row = {"qid": qid, "type": qtype, **s,
                   "retrieved": retrieved[:20], "gold": q["answer_session_ids"],
                   "elapsed_s": round(time.time() - t0, 2)}
            rows.append(row)
            by_type[qtype].append(row)
            print(f"[{i+1}/{len(questions)}] {qid} ({qtype}) "
                  f"R@5={s['r@5']:.0f} R@10={s['r@10']:.0f} "
                  f"MRR={s['mrr']:.3f}  t={row['elapsed_s']}s", file=sys.stderr)
    finally:
        daemon.terminate()
        try:
            daemon.wait(timeout=10)
        except subprocess.TimeoutExpired:
            daemon.kill()
        shutil.rmtree(tmp, ignore_errors=True)
        print(f"# bench daemon stopped, {tmp} removed", file=sys.stderr)

    def avg(rs, k): return sum(r[k] for r in rs) / len(rs) if rs else 0.0
    summary = {
        "overall": {"n": len(rows),
                    **{f"r@{k}": avg(rows, f"r@{k}") for k in K_VALUES},
                    "mrr": avg(rows, "mrr"), "ndcg@10": avg(rows, "ndcg@10")},
        "by_type": {t: {"n": len(rs),
                        **{f"r@{k}": avg(rs, f"r@{k}") for k in K_VALUES},
                        "mrr": avg(rs, "mrr"), "ndcg@10": avg(rs, "ndcg@10")}
                    for t, rs in by_type.items()},
    }
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    json.dump({"summary": summary, "rows": rows}, open(args.out, "w"), indent=2)

    o = summary["overall"]
    print(f"\n## LongMemEval-S — ling-mem (n={o['n']})\n")
    print(f"| R@5 | R@10 | R@20 | MRR | NDCG@10 |\n|---|---|---|---|---|")
    print(f"| {o['r@5']:.1%} | {o['r@10']:.1%} | {o['r@20']:.1%} | {o['mrr']:.3f} | {o['ndcg@10']:.3f} |\n")
    print(f"### By question type\n\n| Type | n | R@5 | R@10 |\n|---|---|---|---|")
    for t, s in sorted(summary["by_type"].items()):
        print(f"| {t} | {s['n']} | {s['r@5']:.1%} | {s['r@10']:.1%} |")


if __name__ == "__main__":
    main()
