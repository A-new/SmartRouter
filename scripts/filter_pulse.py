#!/usr/bin/env python3
"""
Strip blocklisted-host items from pulse JSON files in-place.

The upstream ZH aggregator (SuYxh/ai-news-aggregator) wraps a lot of
individual KOL X/Twitter posts as "news" items — roughly 8% of the 7d
window. They're editorial / promotional rather than reporting, so we drop
them at refresh time. This runs in the GHA workflow after curl-ing the
upstream JSON and again from build_en_pulse.py for symmetry.

Schema-aware: rewrites `total_items` and `items`, leaves the rest of the
payload (site_stats, archive_total, etc.) untouched.
"""

from __future__ import annotations

import json
import re
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

# Match the full host segment (incl. any subdomain). Three hosts:
#   x.com / *.x.com           — KOL posts, not reporting
#   twitter.com / *.twitter.com — same
#   v2ex.com / *.v2ex.com     — forum chatter, very low signal for AI news
BLOCKED_HOST_RE = re.compile(r"^https?://([^/]+\.)?(x|twitter|v2ex)\.com/", re.IGNORECASE)

# Match items whose title is platform moderation/announcement noise OR
# obvious paid placement / promotional content.
#
# - "社区公告"          juejin / similar moderation announcements wrapped as news
# - bracketed markers   【广告】 / 【推广】 / 【赞助】 / 【AD】 / 【PR】 / 【Sponsored】
#                       (and ASCII-bracket variants) — paid placements from KOLs
# - title-prefix promos "广告：…", "推广 | …", "赞助：…" — non-bracketed promo prefixes
#
# Patterns are intentionally narrow so legitimate articles whose body
# discusses advertising/sponsorship (e.g. "Anthropic 拒绝 X 公司赞助",
# "广告业的 AI 转型") do NOT get filtered. Add new shapes as we see them.
BLOCKED_TITLE_RE = re.compile(
    r"(?:"
    r"社区公告"
    r"|[【\[](?:广告|推广|赞助|AD|PR|Sponsored)[】\]]"
    r"|^(?:广告|推广|赞助)[:：\s|]"
    r")",
    re.IGNORECASE,
)


# Some upstream aggregators (newsnow + juejin in particular, a few wechat
# scrapers occasionally) stamp Beijing time as if it were UTC, so a story
# published at 16:24 CST gets written as "2026-05-23T16:24:59Z" — 8h in
# the future relative to the snapshot's own clock. That bad row sorts to
# the top of the user-facing feed while the relative-time label correctly
# shows "10 小时前", which looks broken.
#
# Rewrite the offending published_at to the upstream-recorded
# first_seen_at (which is set when we first crawled the URL, so it's
# always real) so the row lands at its true chronological position.
# 5-minute slack matches the frontend's itemTs() guard.
FUTURE_SLACK = timedelta(minutes=5)


def _parse_iso(value: object) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    # fromisoformat accepts "...+00:00" but not "...Z" pre-3.11; normalize.
    s = value.replace("Z", "+00:00") if value.endswith("Z") else value
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def normalize_future_timestamps(payload: dict) -> int:
    """Rewrite published_at when it sits ahead of the snapshot clock.

    Reference time is the payload's own ``generated_at`` (deterministic
    across workflow re-runs); falls back to wall-clock UTC. Returns the
    count of rewritten items for observability in the action log.
    """
    reference = _parse_iso(payload.get("generated_at")) or datetime.now(timezone.utc)
    cutoff = reference + FUTURE_SLACK
    fixed = 0
    for it in payload.get("items") or []:
        pub = _parse_iso(it.get("published_at"))
        if pub is None or pub <= cutoff:
            continue
        fallback = it.get("first_seen_at") or it.get("last_seen_at")
        if not fallback:
            continue
        it["published_at"] = fallback
        fixed += 1
    return fixed


def filter_file(path: Path) -> tuple[int, int, int]:
    text = path.read_text(encoding="utf-8")
    payload = json.loads(text)
    items = payload.get("items") or []
    before = len(items)
    kept = [
        it
        for it in items
        if not BLOCKED_HOST_RE.match(it.get("url") or "")
        and not BLOCKED_TITLE_RE.search(it.get("title") or "")
    ]
    payload["items"] = kept
    payload["total_items"] = len(kept)
    fixed = normalize_future_timestamps(payload)
    # Preserve the upstream's indent style so refresh commits show only the
    # actual content delta, not a wholesale reformat. SuYxh's ZH files ship
    # with indent=2; our EN builder writes indent=0. Sniff by peeking at the
    # first child line — indented vs flush-left.
    indent = 2 if len(text) > 2 and text[1] == "\n" and text[2] == " " else 0
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=indent), encoding="utf-8")
    return before, len(kept), fixed


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: filter_pulse.py FILE [FILE ...]", file=sys.stderr)
        return 2
    for arg in argv[1:]:
        path = Path(arg)
        before, after, fixed = filter_file(path)
        dropped = before - after
        print(f"{path}: {before} → {after} ({dropped} dropped, {fixed} ts-normalized)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
