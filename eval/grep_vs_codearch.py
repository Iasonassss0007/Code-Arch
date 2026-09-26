"""Context cost of a blast-radius lookup: batched-grep BFS vs one `codearch importers` call.

Tokens are counted with the Gemini countTokens API (GEMINI_API_KEY), not estimated.
usage: python eval/grep_vs_codearch.py REPO FILE
"""
import json, os, re, subprocess, sys, urllib.request
from pathlib import Path

MODEL = "gemini-3.1-flash-lite"
EXT = "{ts,tsx,js,jsx,mjs,cjs,mts,cts,py}"
MAX_ROUNDS = 10


def tokens(text):
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:countTokens"
    body = json.dumps({"contents": [{"parts": [{"text": text}]}]}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json",
                                             "x-goog-api-key": os.environ["GEMINI_API_KEY"]})
    return json.load(urllib.request.urlopen(req))["totalTokens"]


def stem(path):
    p = Path(path)
    name = p.parent.name if p.stem == "index" else p.stem
    return re.escape(name)


def grep_bfs(repo, target):
    """One batched rg per depth over the newest frontier's basenames."""
    seen, frontier = {target}, [target]
    cmd_text = result_text = ""
    rounds = 0
    while frontier and rounds < MAX_ROUNDS:
        rounds += 1
        pat = r"""['"][^'"]*\b(%s)(\.[cm]?[jt]sx?)?['"]""" % "|".join(sorted({stem(f) for f in frontier}))
        cmd = ["rg", "-l", "-g", f"*.{EXT}", "-g", "!node_modules", pat, "."]
        out = subprocess.run(cmd, cwd=repo, text=True, capture_output=True).stdout
        cmd_text += " ".join(cmd[:5] + [f"'{pat}'", "."]) + "\n"
        result_text += out
        hits = {l.removeprefix("./").replace("\\", "/") for l in out.splitlines()}
        frontier = sorted(hits - seen)
        seen |= hits
    return len(seen) - 1, rounds, cmd_text, result_text


def main():
    repo, target = Path(sys.argv[1]), sys.argv[2]
    subprocess.run(["codearch", "."], cwd=repo, check=True, capture_output=True)
    ca_cmd = f"codearch importers {target}"
    ca_out = subprocess.run(["codearch", "importers", target], cwd=repo, text=True, capture_output=True).stdout
    n, rounds, g_cmd, g_out = grep_bfs(repo, target)
    g = tokens(g_cmd) + tokens(g_out)
    c = tokens(ca_cmd) + tokens(ca_out)
    print(f"grep: {rounds} rounds, {n} files, {g} tok | codearch: 1 call, {c} tok | ratio {g / c:.1f}x")


if __name__ == "__main__":
    main()
