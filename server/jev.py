"""Jev, TypeSafe's System One decision model, reached through OpenRouter.

Jev answers narrow typed questions about a piece of state with calibrated
probabilities. ELESSAR asks it one yes/no ("noul") question per signal about
each candidate record, and uses the probability as that record's evidence.

Answers are cached on disk by (model, state, questions), so a replay is
instant, free and deterministic. The API key comes from OPENROUTER_API_KEY
(loaded from .env by app.py) and is never logged. ELESSAR_DECISION_MODEL
picks the decision model (default jev-latest).
"""
import hashlib
import json
import os
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

ENDPOINT = "https://openrouter.ai/api/v1/systemone"
MODEL = os.environ.get("ELESSAR_DECISION_MODEL", "jev-latest")
CACHE_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), ".cache", "jev")
_lock = threading.Lock()


def available():
    return bool(os.environ.get("OPENROUTER_API_KEY"))


def _payload(state, questions):
    return {"model": MODEL, "state": state, "questions": questions}


def _cache_path(payload):
    digest = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()[:32]
    return os.path.join(CACHE_DIR, f"{digest}.json")


def cache_path(state, questions):
    """Where the answer to this exact ask lives, cached or not."""
    return _cache_path(_payload(state, questions))


def load_cached(path):
    """The cached entry at `path`, or None. Entries written before the
    inspector existed carry no payload; they are reported as legacy."""
    if not os.path.isfile(path):
        return None
    with open(path) as f:
        return json.load(f)


def list_cache():
    """Every cache file: {"path", "payload" (may be None), "model", "at", "answers"}."""
    out = []
    if not os.path.isdir(CACHE_DIR):
        return out
    for name in sorted(os.listdir(CACHE_DIR)):
        if not name.endswith(".json"):
            continue
        path = os.path.join(CACHE_DIR, name)
        try:
            with open(path) as f:
                d = json.load(f)
        except (OSError, ValueError):
            continue
        out.append({"path": path, "payload": d.get("payload"), "model": d.get("model"), "at": d.get("at"),
                    "answers": d.get("answers", {})})
    return out


def forget(paths):
    """Delete cache entries. Returns how many were removed."""
    n = 0
    with _lock:
        for p in paths:
            if os.path.isfile(p) and os.path.dirname(os.path.abspath(p)) == os.path.abspath(CACHE_DIR):
                os.remove(p)
                n += 1
    return n


def ask(state, questions):
    """One request: a state and named questions. Returns
    {"answers", "model", "ms", "cost", "cached"} or {"error"}."""
    payload = _payload(state, questions)
    path = _cache_path(payload)
    cached = load_cached(path)
    if cached is not None:
        return {**cached, "cached": True}
    req = urllib.request.Request(
        ENDPOINT,
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {os.environ['OPENROUTER_API_KEY']}",
            "Content-Type": "application/json",
            "X-Title": "ELESSAR",
        },
    )
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            body = json.load(resp)
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")[:300]
        return {"error": f"HTTP {e.code}: {detail}"}
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        return {"error": str(e)}
    result = {
        "payload": payload,
        "answers": body.get("answers", {}),
        "model": body.get("model", MODEL),
        "ms": int((time.time() - t0) * 1000),
        "cost": (body.get("usage") or {}).get("cost", 0.0),
        "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    with _lock:
        os.makedirs(CACHE_DIR, exist_ok=True)
        with open(path, "w") as f:
            json.dump(result, f)
    return {**result, "cached": False}


def ask_many(jobs, workers=8):
    """jobs: list of (key, state, questions). Returns {key: result}, run in parallel."""
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {key: pool.submit(ask, state, questions) for key, state, questions in jobs}
        return {key: f.result() for key, f in futures.items()}
