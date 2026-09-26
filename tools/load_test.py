#!/usr/bin/env python3
"""Concurrent load testing tool for the gh-report dashboard."""

from __future__ import annotations

import argparse
import http.client
import random
import re
import signal
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

CORE_PAGES = [
    "/",
    "/report.html",
    "/owners.html",
    "/orphans.html",
    "/admin.html",
    "/deleted.html",
    "/codeowners.html",
    "/branch_protection.html",
    "/dependabot_status.html",
    "/security_policy.html",
    "/secret_scanning.html",
    "/lifecycle_retirement.html",
    "/alert_free.html",
    "/api/v1/status",
    "/readyz",
    "/style.css",
    "/ws.js",
    "/favicon.svg",
]


class LoadTester:

  def __init__(
      self,
      base_url: str,
      num_users: int = 10,
      walk_interval: float = 5.0,
      auth_token: str | None = None,
      enable_zstd: bool = False,
      keep_alive: bool = True,
  ):
    self.base_url = base_url.rstrip("/")
    parsed = urllib.parse.urlparse(self.base_url)
    self.host = parsed.hostname or "127.0.0.1"
    self.port = parsed.port or (443 if parsed.scheme == "https" else 80)
    self.is_ssl = parsed.scheme == "https"
    self.num_users = num_users
    self.walk_interval = walk_interval
    self.auth_token = auth_token
    self.enable_zstd = enable_zstd
    self.keep_alive = keep_alive
    self.pages = list(CORE_PAGES)
    self.stop_event = threading.Event()

    self.lock = threading.Lock()
    self.requests_count = 0
    self.status_counts = defaultdict(int)
    self.latencies = defaultdict(list)
    self.errors = defaultdict(int)

  def get_connection(self):
    if self.is_ssl:
      import ssl

      ctx = ssl.create_default_context()
      return http.client.HTTPSConnection(
          self.host, self.port, timeout=20, context=ctx
      )
    return http.client.HTTPConnection(self.host, self.port, timeout=20)

  def discover_pages(self):
    """Crawl /owners.html to discover dynamic team drilldown pages."""
    print(
        f"Discovering dynamic drilldown pages from"
        f" {self.base_url}/owners.html...",
        flush=True,
    )
    headers = {"User-Agent": "gh-report-loadtest/discovery"}
    if self.enable_zstd:
      headers["Accept-Encoding"] = "zstd"
    if self.auth_token:
      headers["Authorization"] = f"Bearer {self.auth_token}"

    try:
      req = urllib.request.Request(f"{self.base_url}/owners.html", headers=headers)
      with urllib.request.urlopen(req, timeout=15) as resp:
        data = resp.read()
        if resp.headers.get("Content-Encoding") == "zstd":
          import subprocess

          try:
            data = subprocess.run(
                ["zstd", "-d"], input=data, capture_output=True, check=True
            ).stdout
          except Exception:
            pass
        html = data.decode("utf-8", errors="ignore")
        team_links = set(re.findall(r'href="(owners/[^"#\?]+\.html)"', html))
        for link in sorted(team_links):
          self.pages.append(f"/{link}")
      print(
          f"Discovered {len(team_links)} team drilldown pages. Total pages in"
          f" rotation: {len(self.pages)}",
          flush=True,
      )
    except Exception as e:
      print(
          f"Notice: Page discovery returned {e.__class__.__name__}: {e};"
          f" running with {len(self.pages)} core pages.",
          flush=True,
      )

  def user_worker(self, user_id: int):
    headers = {
        "User-Agent": f"gh-report-loadtest/1.0 (simulated-user-{user_id})",
        "Accept": "text/html,application/xhtml+xml,application/json,*/*",
    }
    if self.enable_zstd:
      headers["Accept-Encoding"] = "zstd"
    if self.auth_token:
      headers["Authorization"] = f"Bearer {self.auth_token}"

    conn = self.get_connection() if self.keep_alive else None

    while not self.stop_event.is_set():
      page = random.choice(self.pages)
      start = time.perf_counter()
      status_code = 0
      try:
        if not self.keep_alive or conn is None:
          conn = self.get_connection()
        conn.request("GET", page, headers=headers)
        resp = conn.getresponse()
        resp.read()
        status_code = resp.status
        if not self.keep_alive:
          conn.close()
          conn = None
      except Exception as e:
        status_code = 0
        with self.lock:
          self.errors[str(e.__class__.__name__)] += 1
        if conn is not None:
          try:
            conn.close()
          except Exception:
            pass
          conn = None

      elapsed_ms = (time.perf_counter() - start) * 1000.0

      with self.lock:
        self.requests_count += 1
        self.status_counts[status_code] += 1
        self.latencies[page].append(elapsed_ms)

      # Wait ~5s per user with ±10% jitter
      jitter = random.uniform(0.9, 1.1)
      self.stop_event.wait(self.walk_interval * jitter)

    if conn is not None:
      try:
        conn.close()
      except Exception:
        pass

  def print_checkpoint(self, elapsed_secs: float, total_duration: float):
    with self.lock:
      reqs = self.requests_count
      rps = reqs / max(elapsed_secs, 1.0)
      ok = self.status_counts[200] + self.status_counts[304]
      errs = sum(
          c for k, c in self.status_counts.items() if k not in (200, 304)
      ) + sum(self.errors.values())
      pct = (elapsed_secs / total_duration) * 100.0

      all_lats = [l for l_list in self.latencies.values() for l in l_list]
      if all_lats:
        p50 = sorted(all_lats)[int(len(all_lats) * 0.50)]
        p95 = sorted(all_lats)[int(len(all_lats) * 0.95)]
      else:
        p50, p95 = 0.0, 0.0

    hours = int(elapsed_secs // 3600)
    mins = int((elapsed_secs % 3600) // 60)
    secs = int(elapsed_secs % 60)
    time_str = f"{hours:02d}h {mins:02d}m {secs:02d}s"

    print(
        f"[{time_str} ({pct:5.1f}%)] Requests: {reqs:<6} | {rps:4.1f} req/s |"
        f" 2xx/3xx: {ok:<6} | Errors: {errs:<3} | p50: {p50:5.1f}ms | p95:"
        f" {p95:5.1f}ms",
        flush=True,
    )

  def run(self, duration_secs: int = 18000, checkpoint_interval: int = 60):
    self.discover_pages()
    duration_hrs = duration_secs / 3600.0
    print(
        f"\nStarting {self.num_users} simulated users walking pages randomly"
        f" every ~{self.walk_interval}s for {duration_secs}s"
        f" ({duration_hrs:.1f} hours)...",
        flush=True,
    )
    print(f"Target URL: {self.base_url}", flush=True)
    print(
        f"Checkpoint interval: every {checkpoint_interval}s\n" + "-" * 75,
        flush=True,
    )

    start_time = time.time()
    last_checkpoint = start_time

    threads = []
    for uid in range(self.num_users):
      t = threading.Thread(target=self.user_worker, args=(uid,), daemon=True)
      t.start()
      threads.append(t)

    try:
      while time.time() - start_time < duration_secs and not self.stop_event.is_set():
        time.sleep(1.0)
        now = time.time()
        if now - last_checkpoint >= checkpoint_interval:
          self.print_checkpoint(now - start_time, duration_secs)
          last_checkpoint = now
    except KeyboardInterrupt:
      print("\nInterrupted by user signal.", flush=True)
    finally:
      self.stop_event.set()
      for t in threads:
        t.join(timeout=1.0)

    total_time = max(time.time() - start_time, 0.001)
    print("\n" + "=" * 75, flush=True)
    print(
        f"LOAD TEST COMPLETE (ran {total_time:.1f}s /"
        f" {total_time / 3600:.2f}h)",
        flush=True,
    )
    print("=" * 75, flush=True)

    with self.lock:
      reqs = self.requests_count
      print(
          f"Total Requests: {reqs} ({reqs / total_time:.2f} req/sec)", flush=True
      )
      print("Status Codes:", flush=True)
      for code, count in sorted(self.status_counts.items()):
        status_label = code if code != 0 else "Network / Timeout Error"
        print(
            f"  HTTP {status_label}: {count} ({count / max(reqs, 1) * 100:.2f}%)",
            flush=True,
        )

      if self.errors:
        print("Exceptions Encountered:", flush=True)
        for err, count in self.errors.items():
          print(f"  {err}: {count}", flush=True)

      all_latencies = [l for l_list in self.latencies.values() for l in l_list]
      if all_latencies:
        all_latencies.sort()
        n = len(all_latencies)
        p50 = all_latencies[int(n * 0.50)]
        p90 = all_latencies[int(n * 0.90)]
        p95 = all_latencies[int(n * 0.95)]
        p99 = all_latencies[int(n * 0.99)]
        print("\nLatency Metrics (Overall):", flush=True)
        print(f"  Min:  {min(all_latencies):6.2f} ms", flush=True)
        print(f"  p50:  {p50:6.2f} ms", flush=True)
        print(f"  p90:  {p90:6.2f} ms", flush=True)
        print(f"  p95:  {p95:6.2f} ms", flush=True)
        print(f"  p99:  {p99:6.2f} ms", flush=True)
        print(f"  Max:  {max(all_latencies):6.2f} ms", flush=True)

      print("\nSlowest Pages by p95 Latency:", flush=True)
      page_stats = []
      for page, lats in self.latencies.items():
        if lats:
          lats_sorted = sorted(lats)
          p95_p = lats_sorted[int(len(lats_sorted) * 0.95)]
          page_stats.append((p95_p, page, len(lats), sum(lats) / len(lats)))
      page_stats.sort(reverse=True)
      for p95_p, page, count, avg_lat in page_stats[:10]:
        print(
            f"  {page:<35} | requests: {count:<5} | avg: {avg_lat:6.2f} ms |"
            f" p95: {p95_p:6.2f} ms",
            flush=True,
        )
    print("=" * 75, flush=True)


def main():
  parser = argparse.ArgumentParser(
      description="Load test gh-report dashboard with simulated concurrent users"
  )
  parser.add_argument(
      "--url", default="http://127.0.0.1:8080", help="Target base URL"
  )
  parser.add_argument(
      "--users", type=int, default=10, help="Number of concurrent users"
  )
  parser.add_argument(
      "--interval",
      type=float,
      default=5.0,
      help="Seconds between requests per user",
  )
  parser.add_argument(
      "--duration",
      type=int,
      default=18000,
      help="Duration in seconds (default: 18000 = 5h)",
  )
  parser.add_argument(
      "--checkpoint",
      type=int,
      default=60,
      help="Checkpoint log interval in seconds (default: 60)",
  )
  parser.add_argument(
      "--auth-token",
      default=None,
      help="Bearer auth token for endpoints behind IAP / Cloud Run ingress",
  )
  parser.add_argument(
      "--zstd",
      action="store_true",
      default=True,
      help="Send Accept-Encoding: zstd to test pre-compressed serving (default: True)",
  )
  parser.add_argument(
      "--no-zstd",
      dest="zstd",
      action="store_false",
      help="Do not send Accept-Encoding: zstd (forces server identity decode)",
  )
  parser.add_argument(
      "--no-keep-alive",
      action="store_true",
      default=False,
      help="Disable HTTP Keep-Alive (open new TCP socket per request)",
  )
  args = parser.parse_args()

  tester = LoadTester(
      base_url=args.url,
      num_users=args.users,
      walk_interval=args.interval,
      auth_token=args.auth_token,
      enable_zstd=args.zstd,
      keep_alive=not args.no_keep_alive,
  )

  def sig_handler(sig, frame):
    tester.stop_event.set()

  signal.signal(signal.SIGINT, sig_handler)
  signal.signal(signal.SIGTERM, sig_handler)

  tester.run(duration_secs=args.duration, checkpoint_interval=args.checkpoint)


if __name__ == "__main__":
  main()
