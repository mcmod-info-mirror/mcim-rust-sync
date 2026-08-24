# mcim-rust-sync

从 Mod 平台拉取信息，写入 MCIM 的缓存库，[mcim-sync](https://github.com/mcmod-info-mirror/mcim-sync) 的 Rust 实现。

每次运行只执行一个任务，跑完即退出，不再内置调度器。

## 缓存思路

[mcim-rust-api](https://github.com/mcmod-info-mirror/mcim-rust-api) 会把所有不存在于数据库中的请求参数写进几个 Redis 集合，其中有的是真的新 Mod 未收录，大部分为无效请求参数。

`queue` 任务定时检查这些 `modId` `fileId` `fingerprint` `project_id` `version_id` `hash`，统一转成 `modId` 与 `project_id` 后拉取，让 MCIM 及时捕捉到新 Mod。

`refresh` 任务定时检查库内所有已缓存的 Mod，以 Modrinth Project 的 `updated` 或 CurseForge Mod 的 `dateModified` 为判据，同步有新版本的条目。

## 用法

```
mcim-rust-sync [-c config.json] [-v] <curseforge|modrinth> <任务>
```

| 任务 | CurseForge | Modrinth |
| --- | --- | --- |
| `queue` | 消费未命中的 modid / fileid / fingerprint | 消费未命中的 project_id / version_id / hash |
| `refresh` | 比对 `dateModified` 增量同步 | 比对 `updated` 与版本列表增量同步，并清理已删除的项目 |
| `refresh-full` | — | 重新同步库内全部项目 |
| `search` | 按发布时间倒序发现新 mod | 按最新发布发现新项目 |
| `categories` / `tags` | 刷新分类 | 刷新 categories、loaders 与 game_versions |

`search` 与 `categories` 默认覆盖 gameId 432 与 78022，可用 `--game-id` 指定其一。

`search` 每翻一页就同步该页发现的新条目，进程中断不会丢掉已同步的部分；`--max-pages` 限制翻页数，缺省 0 表示翻到上游给不出结果为止。

CurseForge 没有 `refresh-full`：生产环境从未真正跑过它，按上游限流跑完 15 万个 mod 需要 28 小时以上，日频调度不可能完成，增量 `refresh` 已覆盖同样的目的。

有条目同步失败时以非零码退出，失败的 id 会被放回 Redis 队列等待下一轮。

## 配置

沿用 mcim-sync 的 `config.json`，`job_config`、`interval`、`cron_trigger` 等调度相关的键不再需要，留着也不会报错。

密钥可以用环境变量覆盖，避免明文落在配置文件里：

| 环境变量 | 覆盖项 |
| --- | --- |
| `MCIM_CURSEFORGE_API_KEY` | `curseforge_api_key` |
| `MCIM_MONGODB_PASSWORD` | `mongodb.password` |
| `MCIM_REDIS_PASSWORD` | `redis.password` |

`curseforge_api` 与 `modrinth_api` 可指向 `cf-api.mcimirror.top` 与 `mr-api.mcimirror.top`，不必直连上游。

## 调度

用 crontab 或 systemd timer 驱动，参考 mcim-sync 的执行频率：

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

## 数据模型

模型与 [mcim-model](https://github.com/mcmod-info-mirror/mcim-model) 及 mcim-rust-api 的 entities 逐字段一致，`tests/models.rs` 用两个方向的夹具验证：`db_*` 来自库内既有数据，`api_*` 来自上游响应。

`file_cdn_cached` 是遗留字段，不再写入。
