#!/usr/bin/env python3
"""
Inject EchoBird-owned pinned items into a pulse JSON feed at fixed positions.

Reads scripts/pulse_pinned.json (a {zh: [...], en: [...]} config) and writes
each pin into the target file at its declared 1-indexed position.

How "position 3" works end-to-end: the Rust client (echobird_core::pulse_archive)
re-sorts items by `effective_ts` (= published_at, with first_seen_at fallback
for future-stamped rows) DESC before rendering. Inserting at array index 2
alone would NOT pin the slot — the Rust sort would rearrange us by timestamp.

So this script computes a `published_at` that lands the pin BETWEEN the
existing items[position-2] (which stays above) and items[position-1] (which
gets pushed down). Concretely: above_ts - 1 second. That guarantees the pin
sorts to exactly the requested position on the Rust side.

Run once per file:
    python scripts/inject_pinned.py docs/pulse/latest-24h.json --lang zh
    python scripts/inject_pinned.py docs/pulse/latest-7d.json  --lang zh
    python scripts/inject_pinned.py docs/pulse/latest-7d-en.json --lang en

Failure mode: any error logs ::warning:: and exits 0 — keeps the refresh
workflow green if our pinned content has a typo or the config is missing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

DEFAULT_CONFIG = Path(__file__).parent / "pulse_pinned.json"
ISO_Z_MS = "%Y-%m-%dT%H:%M:%S.{ms:03d}Z"


def _iso_z_now(dt: datetime) -> str:
    """Format `dt` as the millisecond-precision ISO-Z string Rust expects."""
    dt = dt.astimezone(timezone.utc)
    return dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z"


def _effective_ts_str(item: dict[str, Any]) -> str | None:
    """Mirror Rust effective_ts but return the RAW string — preserves
    upstream sub-second precision so we can match it byte-for-byte. The
    Rust sort compares `&str`, not parsed datetimes, so string identity
    is what places the pin correctly. Skip the future-guard: our pins
    are never future-stamped."""
    for key in ("published_at", "first_seen_at", "last_seen_at"):
        v = item.get(key)
        if v:
            return v.strip()
    return None


def _pre_sort_desc(items: list[dict[str, Any]]) -> None:
    """Reorder `items` in place to match Rust's effective_ts DESC sort.
    Required because upstream's array order is NOT pre-sorted by Rust's
    criteria, so our position-based insertion would otherwise miss its slot."""
    items.sort(key=lambda it: _effective_ts_str(it) or "", reverse=True)


def compute_pin_ts_string(items_sorted: list[dict[str, Any]], position: int) -> str:
    """Return the published_at STRING that lands the pin at sorted position
    `position` (1-indexed) after Rust re-sorts. Caller MUST pass `items` in
    Rust-sort order (use `_pre_sort_desc` first).

    Strategy: inherit the effective_ts of the item that will be directly
    ABOVE the pin after insertion — i.e. items_sorted[position-2]. Combined
    with array insertion at index `position-1`, stable sort places the pin
    one slot below that item, regardless of how many other items share the
    same timestamp value. For position 1, inherit items[0]'s ts and insert
    at index 0 so the pin wins the tiebreak by lower array index."""
    if not items_sorted:
        # Empty feed — shouldn't realistically happen, but bail safely.
        return _iso_z_now(datetime.now(timezone.utc))

    above_idx = max(0, position - 2)
    above_idx = min(above_idx, len(items_sorted) - 1)
    ts = _effective_ts_str(items_sorted[above_idx])
    if ts:
        return ts
    # Item had no parseable ts of its own — fall back to now-5s so we still
    # land at a sensible spot near the top.
    return _iso_z_now(datetime.now(timezone.utc) - timedelta(seconds=5))


def _build_item(pin: dict[str, Any], ts_iso: str, now_iso: str) -> dict[str, Any]:
    url = pin["url"]
    title = pin["title"]
    return {
        # Deterministic id so re-runs don't churn the diff.
        "id": hashlib.sha1(("echobird-pin:" + url).encode("utf-8")).hexdigest(),
        "site_id": pin.get("site_id", "echobird"),
        "site_name": pin.get("site_name", "EchoBird"),
        "source": pin.get("source", "EchoBird"),
        "title": title,
        "url": url,
        "published_at": ts_iso,
        "first_seen_at": now_iso,
        "last_seen_at": now_iso,
        "title_original": title,
        "title_en": pin.get("title_en"),
        "title_zh": pin.get("title_zh", title),
        "title_bilingual": pin.get("title_bilingual", title),
    }


def inject(path: Path, pins: list[dict[str, Any]]) -> tuple[int, int]:
    if not pins:
        return 0, 0

    text = path.read_text(encoding="utf-8")
    payload = json.loads(text)
    items: list[dict[str, Any]] = list(payload.get("items") or [])
    before = len(items)

    # De-dup by URL so the pinned slot is the only place the URL appears.
    pinned_urls = {p["url"] for p in pins}
    items = [it for it in items if it.get("url") not in pinned_urls]

    # Pre-sort to match Rust's effective_ts DESC view. Required for our
    # position-based insertion math to be correct — upstream's array order
    # is not always in Rust-sort order (the cron just appends/merges).
    _pre_sort_desc(items)

    now = datetime.now(timezone.utc)
    now_iso = _iso_z_now(now)
    # Insert in position order so each subsequent pin's "above neighbour"
    # already includes any earlier pins that landed above its slot.
    for pin in sorted(pins, key=lambda x: int(x.get("position", 1))):
        pos = max(1, int(pin.get("position", 1)))
        ts_str = compute_pin_ts_string(items, pos)
        item = _build_item(pin, ts_str, now_iso)
        items.insert(min(pos - 1, len(items)), item)

    payload["items"] = items
    payload["total_items"] = len(items)

    # Bump site_stats so any consumer reading metadata sees coherent counts
    # for our pinned-source rows. Schema differs between feeds: ZH ships a
    # list of {site_id,site_name,count,raw_count} objects (SuYxh upstream),
    # EN ships a flat {site_id: count} dict (our own build_en_pulse.py).
    seen_sids = {p.get("site_id", "echobird") for p in pins}
    site_stats = payload.get("site_stats")
    if isinstance(site_stats, list):
        for sid in seen_sids:
            count = sum(1 for it in items if it.get("site_id") == sid)
            sname = next(
                (p["site_name"] for p in pins if p.get("site_id") == sid), sid
            )
            existing = next((s for s in site_stats if s.get("site_id") == sid), None)
            if existing:
                existing["count"] = count
                existing["raw_count"] = count
                existing["site_name"] = sname
            else:
                site_stats.append(
                    {
                        "site_id": sid,
                        "site_name": sname,
                        "count": count,
                        "raw_count": count,
                    }
                )
        if "site_count" in payload:
            payload["site_count"] = len(site_stats)
    elif isinstance(site_stats, dict):
        for sid in seen_sids:
            count = sum(1 for it in items if it.get("site_id") == sid)
            site_stats[sid] = count
        if "site_count" in payload:
            payload["site_count"] = len(site_stats)

    # Preserve upstream indent style so refresh commits show only the actual
    # content delta. ZH files ship indent=2; EN file ships indent=0. Sniff
    # the second character of the source to detect.
    indent = 2 if len(text) > 2 and text[1] == "\n" and text[2] == " " else 0
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=indent),
        encoding="utf-8",
    )
    return before, len(items)


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("file", type=Path, help="Pulse JSON file to mutate in-place")
    p.add_argument("--lang", required=True, choices=["zh", "en"])
    p.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    args = p.parse_args(argv[1:])

    if not args.file.exists():
        print(f"::warning::{args.file} does not exist, skipping", file=sys.stderr)
        return 0
    if not args.config.exists():
        print(f"::warning::{args.config} missing, skipping pin step", file=sys.stderr)
        return 0

    try:
        cfg = json.loads(args.config.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        print(f"::warning::pinned config parse failed: {e}", file=sys.stderr)
        return 0

    pins = list(cfg.get(args.lang) or [])
    if not pins:
        print(f"{args.file}: no pinned items for lang={args.lang}")
        return 0

    try:
        before, after = inject(args.file, pins)
    except Exception as e:  # noqa: BLE001
        print(f"::warning::inject failed for {args.file}: {e}", file=sys.stderr)
        return 0

    pin_titles = ", ".join(p["title"][:40] for p in pins)
    print(f"{args.file}: {before} → {after} (+{len(pins)} pinned: {pin_titles})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
