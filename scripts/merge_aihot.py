#!/usr/bin/env python3
"""
Merge aihot.virxact.com curated picks into a pulse JSON file in-place.

aihot ships a daily-curated stream (~70-80/day, ~14% selection rate from
~500-600 captures across 168 sources). Each item is injected as a separate
pseudo-source (site_id="aihot", site_name="AI HOT 精选") so the existing
client can attribute the curation source without any schema change.

Items are merged by URL. When the same URL appears in both SuYxh's pool
and aihot's picks the aihot version wins: its titles are LLM-normalized
to clean Chinese and tend to be cleaner than SuYxh's upstream-extracted
titles. Discovery timestamps (first_seen_at/last_seen_at) from the
existing entry are preserved so the waterfall ordering stays honest.

Failure mode: any HTTP / parse error logs a warning and the script exits
with code 0 — the workflow proceeds with SuYxh-only items, so aihot
downtime never breaks the refresh cycle.
"""

from __future__ import annotations

import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import feedparser  # type: ignore[import-untyped]
import requests

AIHOT_FEED_URL = "https://aihot.virxact.com/feed.xml"
HTTP_TIMEOUT = 20
USER_AGENT = "EchoBird-PulseBuilder/1.0 (+https://github.com/edison7009/EchoBird)"


def fetch_aihot_entries() -> list[Any]:
    try:
        r = requests.get(
            AIHOT_FEED_URL,
            headers={"User-Agent": USER_AGENT, "Accept": "application/rss+xml"},
            timeout=HTTP_TIMEOUT,
        )
        r.raise_for_status()
    except requests.RequestException as e:
        print(f"::warning::aihot fetch failed: {e}", file=sys.stderr)
        return []
    feed = feedparser.parse(r.content)
    if feed.bozo:
        print(
            f"::warning::aihot feed parse warning: {feed.bozo_exception}",
            file=sys.stderr,
        )
    return list(feed.entries)


def _iso_z(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%dT%H:%M:%S.000Z")


def to_news_item(entry: Any, now_iso: str) -> dict[str, Any] | None:
    url = (entry.get("link") or "").strip()
    title = (entry.get("title") or "").strip()
    if not url or not title:
        return None

    pub_at: str | None = None
    pp = entry.get("published_parsed")
    if pp:
        try:
            pub_at = _iso_z(datetime(*pp[:6], tzinfo=timezone.utc))
        except (TypeError, ValueError):
            pub_at = entry.get("published") or None

    # Stable id: prefer the feed's guid, fall back to a hash of the URL so
    # re-runs keep the same id for the same item.
    guid = entry.get("id") or entry.get("guid") or url
    item_id = hashlib.sha1(guid.encode("utf-8")).hexdigest()

    # aihot's <author> looks like "noreply@aihot.virxact.com (IT之家（RSS）)" —
    # the parenthesised part is the original source name, which is what
    # users want to see, not the noreply address.
    author = (entry.get("author") or "").strip()
    if "(" in author and author.endswith(")"):
        source = author.rsplit("(", 1)[1].rstrip(")").strip() or "AI HOT"
    else:
        source = author or "AI HOT"

    return {
        "id": item_id,
        "site_id": "aihot",
        "site_name": "AI HOT 精选",
        "source": source,
        "title": title,
        "url": url,
        "published_at": pub_at,
        "first_seen_at": now_iso,
        "last_seen_at": now_iso,
        "title_original": title,
        "title_zh": title,
        "title_en": None,
        "title_bilingual": title,
    }


def merge_in_place(path: Path) -> tuple[int, int, int, int]:
    text = path.read_text(encoding="utf-8")
    payload = json.loads(text)
    existing: list[dict[str, Any]] = list(payload.get("items") or [])
    before_count = len(existing)

    # Build URL→first-occurrence index of SuYxh items WITHOUT collapsing
    # internal duplicates: SuYxh's stream can list the same URL under
    # different aggregator sites (Buzzing + TopHub, etc.) and that
    # multiplicity is meaningful to the upstream. We leave it intact and
    # only treat the first occurrence as the override target when aihot
    # has a cleaner title for that URL.
    suyxh_first_idx: dict[str, int] = {}
    for idx, it in enumerate(existing):
        u = it.get("url") or ""
        if u and u not in suyxh_first_idx:
            suyxh_first_idx[u] = idx

    now_iso = _iso_z(datetime.now(timezone.utc))
    entries = fetch_aihot_entries()

    added = 0
    overrode = 0
    for entry in entries:
        item = to_news_item(entry, now_iso)
        if not item:
            continue
        url = item["url"]
        if url in suyxh_first_idx:
            # Same URL: keep SuYxh's discovery timing, swap to aihot's
            # cleaner title and re-attribute to the aihot source so the
            # curation signal survives the merge. Only the first occurrence
            # is rewritten — duplicates downstream are left as-is.
            existing_item = existing[suyxh_first_idx[url]]
            existing_item["title"] = item["title"]
            existing_item["title_zh"] = item["title"]
            existing_item["title_original"] = item["title"]
            existing_item["title_bilingual"] = item["title"]
            existing_item["site_id"] = "aihot"
            existing_item["site_name"] = "AI HOT 精选"
            overrode += 1
        else:
            existing.append(item)
            suyxh_first_idx[url] = len(existing) - 1
            added += 1

    merged = existing
    payload["items"] = merged
    payload["total_items"] = len(merged)

    # Keep site_stats internally consistent: SuYxh already ships an "aihot"
    # row with count:0; bump it to reality so any downstream consumer that
    # reads metadata sees a coherent picture.
    site_stats = payload.get("site_stats") or []
    aihot_count = sum(1 for it in merged if it.get("site_id") == "aihot")
    bumped = False
    for s in site_stats:
        if s.get("site_id") == "aihot":
            s["count"] = aihot_count
            s["raw_count"] = aihot_count
            s["site_name"] = "AI HOT 精选"
            bumped = True
            break
    if not bumped and aihot_count > 0:
        site_stats.append(
            {
                "site_id": "aihot",
                "site_name": "AI HOT 精选",
                "count": aihot_count,
                "raw_count": aihot_count,
            }
        )
        payload["site_count"] = len(site_stats)
    payload["site_stats"] = site_stats

    # Preserve upstream indent style so refresh commits show only the
    # actual content delta, not a wholesale reformat. SuYxh's ZH files
    # ship with indent=2; sniff the second char to detect.
    indent = 2 if len(text) > 2 and text[1] == "\n" and text[2] == " " else 0
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=indent),
        encoding="utf-8",
    )
    return before_count, len(merged), added, overrode


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: merge_aihot.py FILE [FILE ...]", file=sys.stderr)
        return 2
    for arg in argv[1:]:
        path = Path(arg)
        if not path.exists():
            print(f"::warning::{path} does not exist, skipping", file=sys.stderr)
            continue
        before, after, added, overrode = merge_in_place(path)
        print(
            f"{path}: {before} → {after} "
            f"(+{added} new from aihot, {overrode} title-overrode)"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
