#!/usr/bin/env python3
"""把本地 mcim-rust-api 的响应与生产镜像逐字段比对。

用来验证本仓库写进 MongoDB 的文档，读侧能原样读出来。

    scripts/compare-api.py --limit 8

差异分两类：
  结构差异 —— 键集合或类型不同，说明写入的文档形状不对，必须修
  取值差异 —— 键与类型一致但值不同，多半是两边同步时刻不同，属正常
"""
import argparse
import json
import subprocess
import sys
from collections import defaultdict

# 两边同步时刻不同必然会变的字段，整棵子树都跳过
#
# 像 latestFiles / latestFilesIndexes 这种「最新文件」内容随发版变化，
# 两边同步时刻不同则条目本身就不是同一批，比结构也没有意义
VOLATILE = {
    "sync_at", "downloadCount", "downloads", "thumbsUpCount", "gamePopularityRank",
    "followers", "dateModified", "updated", "rating", "color", "isAvailable",
    "latestFiles", "latestFilesIndexes", "mainFileId", "dateReleased", "status",
    "versions", "game_versions", "loaders", "categories", "additional_categories",
    "body", "gallery", "description", "title", "summary", "name", "slug",
    "fileStatus", "displayIndex", "icon_url", "iconUrl", "url", "logo",
    "screenshots", "authors", "links", "date_modified", "published",
}


def fetch(url, body=None, timeout=90):
    cmd = ["curl", "-s", "--max-time", str(timeout), url]
    if body is not None:
        cmd += ["-X", "POST", "-H", "Content-Type: application/json", "-d", json.dumps(body)]
    out = subprocess.run(cmd, capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip():
        return None
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return None


def mongo(database, script):
    out = subprocess.run(
        ["mongosh", database, "--quiet", "--eval", script],
        capture_output=True, text=True,
    )
    return [x for x in out.stdout.strip().split(",") if x]


def sort_key(item):
    """列表两边顺序可能不同，按稳定标识排序后再比"""
    if isinstance(item, dict):
        for k in ("id", "_id", "fileId", "project_id", "name", "version"):
            if k in item and isinstance(item[k], (str, int)):
                return (0, str(item[k]))
    return (1, json.dumps(item, sort_keys=True)[:80])


def is_error(payload):
    """两边任意一侧返回错误响应，说明这条只有一边有，不是结构问题"""
    return isinstance(payload, dict) and "code" in payload and "error" in payload


def walk(a, b, path, struct, value, volatile=False):
    # 可空字段从 null 变成有值（或反之）是取值变化，不是结构不一致
    if a is None or b is None:
        if a is not b and not volatile:
            value.append(f"{path}: {a!r} vs {b!r}")
        return
    if type(a) is not type(b) and not (isinstance(a, (int, float)) and isinstance(b, (int, float))):
        struct.append(f"{path}: 类型 {type(a).__name__} vs {type(b).__name__}")
        return
    if isinstance(a, dict):
        for k in sorted(set(a) | set(b)):
            if k in VOLATILE or volatile:
                continue
            if k not in a:
                struct.append(f"{path}.{k}: 本地缺失")
            elif k not in b:
                struct.append(f"{path}.{k}: 生产缺失")
            else:
                walk(a[k], b[k], f"{path}.{k}", struct, value, volatile)
    elif isinstance(a, list):
        n = min(len(a), len(b))
        if len(a) != len(b):
            value.append(f"{path}: 长度 {len(a)} vs {len(b)}")
        for i, (x, y) in enumerate(zip(sorted(a, key=sort_key)[:n], sorted(b, key=sort_key)[:n])):
            walk(x, y, f"{path}[{i}]", struct, value, volatile)
    elif a != b and not volatile:
        value.append(f"{path}: {a!r} vs {b!r}")


def build_targets(database, limit):
    # 随机取样，按自然顺序取前 N 条只会覆盖最早写入的那批
    mods = mongo(database, f"db.curseforge_mods.aggregate([{{$sample:{{size:{limit}}}}},{{$project:{{_id:1}}}}]).toArray().map(d=>d._id).join(',')")
    projects = mongo(database, f"db.modrinth_projects.aggregate([{{$sample:{{size:{limit}}}}},{{$project:{{_id:1}}}}]).toArray().map(d=>d._id).join(',')")
    targets = []
    for m in mods:
        targets.append({"label": f"CF mod {m}", "path": f"/curseforge/v1/mods/{m}", "unwrap": "data"})
        targets.append({"label": f"CF files of {m}", "path": f"/curseforge/v1/mods/{m}/files", "unwrap": "data"})
    if mods:
        targets.append({"label": "CF mods 批量", "path": "/curseforge/v1/mods",
                        "body": {"modIds": [int(x) for x in mods]}, "unwrap": "data"})
    for game_id in (432, 78022):
        targets.append({"label": f"CF categories {game_id}",
                        "path": f"/curseforge/v1/categories?gameId={game_id}", "unwrap": "data"})
    for p in projects:
        targets.append({"label": f"MR project {p}", "path": f"/modrinth/v2/project/{p}"})
        targets.append({"label": f"MR versions of {p}", "path": f"/modrinth/v2/project/{p}/version"})
    for tag in ("category", "loader", "game_version"):
        targets.append({"label": f"MR tag/{tag}", "path": f"/modrinth/v2/tag/{tag}"})
    return targets


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--local", default="http://127.0.0.1:8080")
    parser.add_argument("--prod", default="https://mod.mcimirror.top")
    parser.add_argument("--database", default="mcim_backend")
    parser.add_argument("--limit", type=int, default=8, help="每个平台各取多少条来比对")
    args = parser.parse_args()

    targets = build_targets(args.database, args.limit)
    if not targets:
        print("库里没有数据可比对", file=sys.stderr)
        return 2

    total_struct = total_value = 0
    only_local = only_prod = 0
    by_field = defaultdict(int)
    print(f"{'端点':<52}{'结构':>7}{'取值':>7}")
    print("-" * 66)
    for t in targets:
        local, prod = fetch(args.local + t["path"], t.get("body")), fetch(args.prod + t["path"], t.get("body"))
        if local is None or prod is None:
            print(f"{t['label']:<52}{'请求失败':>14}")
            continue
        # 一侧 404 说明这条只有另一侧有，单独统计而不是当成字段差异
        if is_error(local) or is_error(prod):
            if is_error(prod) and not is_error(local):
                only_local += 1
                print(f"{t['label']:<52}{'仅本地有':>14}")
            elif is_error(local) and not is_error(prod):
                only_prod += 1
                print(f"{t['label']:<52}{'仅生产有':>14}")
            else:
                print(f"{t['label']:<52}{'两边都没有':>13}")
            continue
        if t.get("unwrap"):
            local = local.get(t["unwrap"], local) if isinstance(local, dict) else local
            prod = prod.get(t["unwrap"], prod) if isinstance(prod, dict) else prod
        struct, value = [], []
        walk(local, prod, "", struct, value)
        total_struct += len(struct)
        total_value += len(value)
        for s in struct:
            by_field[s.split(":")[0].split("[")[0]] += 1
        print(f"{t['label']:<52}{len(struct):>7}{len(value):>7}{'  ✗' if struct else ''}")
        for s in struct[:5]:
            print(f"      结构! {s}")
    print("-" * 66)
    print(f"{'合计':<52}{total_struct:>7}{total_value:>7}")
    print(f"仅本地有 {only_local} 个端点，仅生产有 {only_prod} 个端点")
    if by_field:
        print("\n结构差异汇总:")
        for k, v in sorted(by_field.items(), key=lambda x: -x[1])[:15]:
            print(f"  {v:>4}  {k}")
    return 1 if total_struct else 0


if __name__ == "__main__":
    sys.exit(main())
