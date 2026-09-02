# mcim-rust-sync

从 Mod 平台拉取信息，写入 MCIM 的缓存库，[mcim-sync](https://github.com/mcmod-info-mirror/mcim-sync) 的 Rust 实现。

> [!WARNING]
> WIP! Claude Code 含量过高，请勿于生产环境部署

## 关于缓存思路

[mcim-rust-api](https://github.com/mcmod-info-mirror/mcim-rust-api) 会把所有不存在于数据库中的请求参数写进几个 Redis 集合，其中有的是真的新 Mod 未收录，大部分为无效请求参数。

`queue` 任务定时检查这些 `modId` `fileId` `fingerprint` `project_id` `version_id` `hash`，统一转成 `modId` 与 `project_id` 后拉取，让 MCIM 及时捕捉到新 Mod。

`refresh` 任务定时检查库内所有已缓存的 Mod，以 Modrinth Project 的 `updated` 或 CurseForge Mod 的 `dateModified` 为判据，同步有新版本的条目。

## 用法

```
mcim-rust-sync [-c config.json] [-v] <curseforge|modrinth> <task>
```

| Task | CurseForge | Modrinth |
| --- | --- | --- |
| `queue` | 消费未命中的 `modid` / `fileid` / `fingerprint` | 消费未命中的 `project_id` / `version_id` / `hash` |
| `refresh` | 比对 `dateModified` 增量同步 | 比对 `updated` 与版本列表增量同步，并清理已删除的项目 |
| `refresh-full` | — | 重新同步库内全部项目 |
| `search` | 按发布时间倒序发现新 Mod | 按最新发布发现新项目 |
| `categories` / `tags` | 刷新分类 | 刷新 `categories`、`loaders` 与 `game_versions` |

`search` 与 `categories` 默认覆盖 `gameId` `432` 与 `78022`，可用 `--game-id` 指定其一。

`search` 每翻一页就同步该页发现的新条目。`--max-pages` 限制翻页数，缺省 0 表示翻到上游给不出结果为止。初次同步时加 `--full`，遇到已入库的条目也继续同步。

CurseForge 的搜索接口限制 `index + pageSize <= 10000`，单次查询最多只能看到一万条。`--full` 会先扫完各个 class，再按分类逐个补，并且每个分片正反两个方向各扫一遍，倒序给的是最新的一万条、正序给的是最旧的一万条，尽可能获取足够数据。（注意 `classId` 与 `categoryId` 是两个不同的查询参数）

（`search` 是为了捕获新发布的 mod，持续翻页直到不再出现新条目为止，`--full` 的全量扫描只是附带用法。）

CurseForge 没有 `refresh-full`，成本过高。

Exit Code：`0` 同步成功，`1` 有个别条目没同步成功，`2` 整体失败。

失败的 id 会被放回 Redis 队列等待下一轮。

创建数据库后，先执行一次 `mcim-rust-sync indexes` 建立索引。

## 配置

密钥可以用环境变量覆盖，避免明文落在配置文件里：

| 环境变量 | 覆盖项 |
| --- | --- |
| `MCIM_CURSEFORGE_API_KEY` | `curseforge_api_key` |
| `MCIM_MONGODB_PASSWORD` | `mongodb.password` |
| `MCIM_REDIS_PASSWORD` | `redis.password` |

## 调度

`daemon` 常驻，按配置里的 `schedule` 定时执行任务：

```json
"schedule": {
    "curseforge-queue":      { "cron": "*/20 * * * *",     "args": "curseforge queue" },
    "modrinth-queue":        { "cron": "10,30,50 * * * *", "args": "modrinth queue" },
    "curseforge-refresh":    { "cron": "0 */2 * * *",      "args": "curseforge refresh" },
    "modrinth-refresh":      { "cron": "0 */2 * * *",      "args": "modrinth refresh" },
    "curseforge-search":     { "cron": "0 */2 * * *",      "args": "curseforge search" },
    "modrinth-search":       { "cron": "30 */2 * * *",     "args": "modrinth search" },
    "curseforge-categories": { "cron": "0 0 * * *",        "args": "curseforge categories" },
    "modrinth-tags":         { "cron": "0 0 * * *",        "args": "modrinth tags" },
    "modrinth-refresh-full": { "cron": "0 4 * * *",        "args": "modrinth refresh-full" }
}
```

键是任务名，只用于日志；`args` 与命令行子命令写法一致，cron 按 UTC 判定。`--full` 是冷启动用的，不要排进 `schedule`。

同一个任务不自我重叠，上一轮没跑完就跳过本轮并告警；不同任务并发，对上游的速率由按域名共享的令牌桶约束。停机期间错过的班次不补跑。

`SIGTERM` / `SIGINT` 后不再排新任务，等在跑的收尾，超过 `shutdown_grace_secs`（缺省 300 秒）强行终止。守护模式常驻，Exit Code 只区分正常停止（`0`）与出错（`2`），持续故障看日志里的 `streak`。

也可以留空 `schedule`，用 crontab 或 systemd timer 逐个调起：

```cron
*/20 * * * *  mcim-rust-sync curseforge queue
10,30,50 * * * *  mcim-rust-sync modrinth queue
0 */2 * * *   mcim-rust-sync curseforge refresh
0 */2 * * *   mcim-rust-sync modrinth refresh
0 */2 * * *   mcim-rust-sync curseforge search
30 */2 * * *  mcim-rust-sync modrinth search
0 0 * * *     mcim-rust-sync curseforge categories
0 0 * * *     mcim-rust-sync modrinth tags
0 4 * * *     mcim-rust-sync modrinth refresh-full
```

## 鸣谢

谢谢来自 [HyacinthHaru](https://github.com/HyacinthHaru) 的支持！

之前有过尝试，最终在 HyacinthHaru 的支持下才重新开始。
