#!/usr/bin/env python3
"""全量同步跑完之后的验收。

    scripts/verify-sync.py [--sample 300]

做四件事：
  1. 本地与生产的收录量对比
  2. 双向覆盖差异：本地有生产没有的，以及反过来
  3. 抽样核对写入文档的关键不变量
  4. 汇总同步过程中的失败
"""
import argparse
import json
import subprocess
import sys
import urllib.parse

PROD = "https://mod.mcimirror.top"


def mongo(database, script):
    out = subprocess.run(
        ["mongosh", database, "--quiet", "--eval", script],
        capture_output=True, text=True,
    )
    return out.stdout.strip()


def curl(url, body=None, timeout=90):
    cmd = ["curl", "-s", "--max-time", str(timeout), url]
    if body is not None:
        cmd += ["-X", "POST", "-H", "Content-Type: application/json", "-d", json.dumps(body)]
    out = subprocess.run(cmd, capture_output=True, text=True)
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return None


def section(title):
    print(f"\n{'=' * 60}\n{title}\n{'=' * 60}")


def counts(database):
    section("1. 收录量")
    script = (
        "print(['curseforge_mods','curseforge_files','modrinth_projects',"
        "'modrinth_versions','modrinth_files','curseforge_categories',"
        "'modrinth_categories','modrinth_loaders','modrinth_game_versions']"
        ".map(c=>c+'='+db.getCollection(c).countDocuments()).join(','))"
    )
    local = dict(x.split("=") for x in mongo(database, script).split(","))
    stats = curl(f"{PROD}/statistics") or {}
    print(f"  {'集合':<26}{'本地':>10}{'生产':>10}")
    pairs = [
        ("curseforge_mods", stats.get("curseforge", {}).get("mod")),
        ("curseforge_files", stats.get("curseforge", {}).get("file")),
        ("modrinth_projects", stats.get("modrinth", {}).get("project")),
        ("modrinth_versions", stats.get("modrinth", {}).get("version")),
        ("modrinth_files", stats.get("modrinth", {}).get("file")),
    ]
    for name, prod in pairs:
        mine = int(local.get(name, 0))
        p = f"{prod:,}" if isinstance(prod, int) else "-"
        delta = f"  ({mine - prod:+,})" if isinstance(prod, int) else ""
        print(f"  {name:<26}{mine:>10,}{p:>10}{delta}")
    for name in ("curseforge_categories", "modrinth_categories", "modrinth_loaders", "modrinth_game_versions"):
        print(f"  {name:<26}{int(local.get(name, 0)):>10,}")
    return local


def coverage(database, sample):
    section("2. 双向覆盖差异")

    ids = mongo(database, f"db.curseforge_mods.aggregate([{{$sample:{{size:{sample}}}}},{{$project:{{_id:1}}}}]).toArray().map(d=>d._id).join(',')")
    ids = [int(x) for x in ids.split(",") if x]
    got = curl(f"{PROD}/curseforge/v1/mods", {"modIds": ids}) or {}
    have = {m["id"] for m in got.get("data", [])}
    miss = [i for i in ids if i not in have]
    print(f"  CF: 抽本地 {len(ids)} 个，生产缺 {len(miss)} 个 ({len(miss) * 100 // max(len(ids), 1)}%)")
    if miss[:5]:
        print(f"      例: {miss[:5]}")

    pids = mongo(database, f"db.modrinth_projects.aggregate([{{$sample:{{size:{sample}}}}},{{$project:{{_id:1}}}}]).toArray().map(d=>d._id).join(',')")
    pids = [x for x in pids.split(",") if x]
    q = urllib.parse.quote(json.dumps(pids))
    got = curl(f"{PROD}/modrinth/v2/projects?ids={q}")
    have = {p["id"] for p in got} if isinstance(got, list) else set()
    miss = [i for i in pids if i not in have]
    print(f"  MR: 抽本地 {len(pids)} 个，生产缺 {len(miss)} 个 ({len(miss) * 100 // max(len(pids), 1)}%)")
    if miss[:5]:
        print(f"      例: {miss[:5]}")


def invariants(database):
    section("3. 写入文档的关键不变量")
    checks = [
        ("curseforge_mods 的 _id 是整数",
         "db.curseforge_mods.countDocuments({_id:{$not:{$type:'int'}}})", 0),
        ("curseforge_mods 的 sync_at 是 BSON 时间",
         "db.curseforge_mods.countDocuments({sync_at:{$not:{$type:'date'}}})", 0),
        ("curseforge_files 的 modId 都在",
         "db.curseforge_files.countDocuments({modId:{$exists:false}})", 0),
        ("modrinth_files 的 _id 是子文档",
         "db.modrinth_files.countDocuments({_id:{$not:{$type:'object'}}})", 0),
        ("modrinth_files 没有遗留的 file_cdn_cached",
         "db.modrinth_files.countDocuments({file_cdn_cached:{$exists:true}})", 0),
        ("modrinth_versions 的 project_id 都在",
         "db.modrinth_versions.countDocuments({project_id:{$exists:false}})", 0),
        ("modrinth_projects 的 sync_at 是 BSON 时间",
         "db.modrinth_projects.countDocuments({sync_at:{$not:{$type:'date'}}})", 0),
    ]
    bad = 0
    for label, script, expect in checks:
        got = mongo(database, f"print({script})")
        ok = got.isdigit() and int(got) == expect
        bad += 0 if ok else 1
        print(f"  {'✓' if ok else '✗'} {label:<38} 实际 {got}")

    print("\n  modrinth_files 主键字段顺序抽样:")
    order = mongo(database, "print(Object.keys(db.modrinth_files.findOne()._id).join(','))")
    ok = order == "sha512,sha1"
    bad += 0 if ok else 1
    print(f"  {'✓' if ok else '✗'} _id 键序 = {order}（应为 sha512,sha1）")
    return bad


def failures():
    section("4. 同步过程中的失败")
    out = subprocess.run(
        ["bash", "-c",
         "cat /root/mcim/log/*.log 2>/dev/null | sed 's/\\x1b\\[[0-9;]*m//g' | grep '同步失败' "
         "| grep -oE '(project_id|mod_id)=\"?[A-Za-z0-9]+\"?' | sort -u"],
        capture_output=True, text=True)
    ids = [x for x in out.stdout.strip().split("\n") if x]
    print(f"  不同的失败条目: {len(ids)}")
    for i in ids[:10]:
        print(f"    {i}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--database", default="mcim_backend")
    parser.add_argument("--sample", type=int, default=300)
    args = parser.parse_args()

    counts(args.database)
    coverage(args.database, args.sample)
    bad = invariants(args.database)
    failures()
    section("结论")
    print("  不变量检查全部通过" if bad == 0 else f"  有 {bad} 项不变量检查未通过")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
