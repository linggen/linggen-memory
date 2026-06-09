#!/usr/bin/env python3
"""
ling-mem write-scale stress test.

Replicates the LongMemEval runner's WRITE path (one `add --stdin` of ~670
turn-chunks per question into ONE growing store, NO reset) but:
  - captures the daemon's stdout+stderr to a log file (RUST_BACKTRACE=full,
    RUST_LOG=debug) so the REAL error at the failure point is recoverable;
  - logs cumulative rows / per-question latency / daemon RSS each step;
  - on the first failed add, dumps the daemon log tail + full CLI stderr,
    retries twice to confirm the wedge is persistent, then STOPS;
  - LEAVES the daemon up and the temp store intact for inspection.

Usage:
  python scale_test.py --dataset data/longmemeval_s.jsonl --port 19890
"""
import argparse, datetime, json, subprocess, sys, tempfile, time, urllib.request, uuid
from pathlib import Path

CHUNK_CHARS, CHUNK_OVERLAP = 1600, 200


def turn_chunks(turns):
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


def rows_for(q, ctx, now):
    out = []
    for sid, turns in zip(q["haystack_session_ids"], q["haystack_sessions"]):
        for chunk in turn_chunks(turns):
            out.append(json.dumps({
                "id": str(uuid.uuid4()), "content": chunk, "type": "fact",
                "from": "derived", "contexts": [ctx], "tags": [f"session/{sid}"],
                "created_at": now,
            }))
    return out


def daemon_healthy(port):
    try:
        with urllib.request.urlopen(f"http://localhost:{port}/api/health", timeout=3) as r:
            return b'"ok":true' in r.read()
    except Exception:
        return False


def rss_mb(pid):
    try:
        out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                             capture_output=True, text=True).stdout.strip()
        return round(int(out) / 1024, 1) if out else None
    except Exception:
        return None


def add(data_dir, port, ndjson):
    env = {"LING_MEM_NO_TELEMETRY": "1", "LINGGEN_DATA_DIR": str(data_dir),
           "PATH": __import__("os").environ["PATH"]}
    p = subprocess.run(["ling-mem", "--data-dir", str(data_dir), "add", "--stdin", "--quiet"],
                       env=env, input=ndjson, capture_output=True, text=True, timeout=2400)
    if p.returncode != 0:
        raise RuntimeError(p.stderr.strip())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--port", type=int, default=19890)
    ap.add_argument("--limit", type=int, default=60)
    args = ap.parse_args()

    questions = [json.loads(l) for l in open(args.dataset) if l.strip()][:args.limit]
    tmp = Path(tempfile.mkdtemp(prefix="lme-scale-"))
    dlog = Path("results/scale_daemon.log")
    dlog.parent.mkdir(exist_ok=True)
    dfh = open(dlog, "w")

    import os
    env = {**os.environ, "LING_MEM_NO_TELEMETRY": "1", "LINGGEN_DATA_DIR": str(tmp),
           "RUST_BACKTRACE": "full", "RUST_LOG": "debug"}
    print(f"# store: {tmp}", flush=True)
    print(f"# daemon log: {dlog}", flush=True)
    daemon = subprocess.Popen(["ling-mem", "serve", "--port", str(args.port)],
                              env=env, stdout=dfh, stderr=subprocess.STDOUT)
    for _ in range(30):
        if daemon_healthy(args.port):
            break
        time.sleep(0.5)
    else:
        print("# daemon failed to bind", flush=True); sys.exit(1)
    print(f"# daemon up pid={daemon.pid} port={args.port}", flush=True)

    cum = 0
    for i, q in enumerate(questions):
        now = datetime.datetime.now(datetime.timezone.utc).isoformat()
        nd = "\n".join(rows_for(q, f"q_{q['question_id']}", now)) + "\n"
        n = nd.count("\n")
        t0 = time.time()
        try:
            add(tmp, args.port, nd)
            cum += n
            dt = time.time() - t0
            print(f"[{i+1}/{len(questions)}] +{n} rows  cum={cum}  "
                  f"{dt:.1f}s  ({n/dt:.1f} rows/s)  rss={rss_mb(daemon.pid)}MB", flush=True)
        except Exception as e:
            dt = time.time() - t0
            print(f"\n!!! ADD FAILED at q{i+1} after {dt:.1f}s  "
                  f"(was adding {n} rows; cum BEFORE this = {cum})", flush=True)
            print(f"--- CLI stderr ---\n{e}\n", flush=True)
            time.sleep(1)
            print("--- daemon log tail (last 60 lines) ---", flush=True)
            tail = dlog.read_text(errors="replace").splitlines()[-60:]
            print("\n".join(tail), flush=True)
            print("\n--- confirming persistence (2 retries w/ a tiny 1-row add) ---", flush=True)
            for r in range(2):
                try:
                    add(tmp, args.port, json.dumps({
                        "id": str(uuid.uuid4()), "content": "probe", "type": "fact",
                        "from": "derived", "contexts": ["probe"], "tags": ["probe"],
                        "created_at": now}) + "\n")
                    print(f"  retry {r+1}: OK (recovered!)", flush=True)
                except Exception as e2:
                    print(f"  retry {r+1}: still failing -> {str(e2)[:160]}", flush=True)
                time.sleep(1)
            print(f"\n# daemon LEFT RUNNING pid={daemon.pid} port={args.port} "
                  f"store={tmp}", flush=True)
            print(f"# health now: {daemon_healthy(args.port)}", flush=True)
            return
    print(f"\n# completed all {len(questions)} questions, cum={cum} rows, NO failure.", flush=True)
    print(f"# daemon LEFT RUNNING pid={daemon.pid} port={args.port} store={tmp}", flush=True)


if __name__ == "__main__":
    main()
