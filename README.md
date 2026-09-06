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

`refresh` 每轮会给核对过的条目写上 `checked_at`，不管内容有没有变；`sync_at` 只在真的重新拉取时才更新。两者的差就是「确认过还是最新的，但没有变化」。

Exit Code：`0` 同步成功，`1` 有个别条目没同步成功，`2` 整体失败。

失败的 id 会被放回 Redis 队列等待下一轮。

创建数据库后，先执行一次 `mcim-rust-sync indexes` 建立索引。

## 配置

每个配置项都能用环境变量覆盖，容器里可以不挂 `config.json`。文件不存在就全部走默认值与环境变量，文件在但解析不了则直接失败。

| 环境变量 | 覆盖项 |
| --- | --- |
| `DEBUG` | `debug` |
| `MONGODB_HOST` / `_PORT` / `_AUTH` / `_USER` / `_PASSWORD` / `_DATABASE` | `mongodb.*` |
| `REDIS_HOST` / `_PORT` / `_PASSWORD` / `_DATABASE` | `redis.*` |
| `MAX_WORKERS` | `max_workers` |
| `CURSEFORGE_CHUNK_SIZE` / `MODRINTH_CHUNK_SIZE` | 批量接口的分块大小 |
| `CURSEFORGE_API` / `MODRINTH_API` | 上游地址 |
| `CURSEFORGE_API_KEY` | `curseforge_api_key` |
| `PROXY` | `proxy` |
| `SHUTDOWN_GRACE_SECS` | `shutdown_grace_secs` |
| `TASK_API_ADDR` | 任务历史 API 监听地址，默认 `0.0.0.0:9901` |
| `DOMAIN_RATE_LIMITS` | `domain_rate_limits`，整段 JSON |
| `SCHEDULE` | `schedule`，整段 JSON |

映射类型只能整体覆盖，标量按项覆盖。空字符串按未设置处理，写错的值在启动时就报错。

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

`SIGTERM` / `SIGINT` 后不再排新任务，等在跑的收尾，超过 `shutdown_grace_secs`（缺省 60 秒）强行终止。守护模式常驻，Exit Code 只区分正常停止（`0`）与出错（`2`），持续故障看日志里的 `streak`。

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

## Prometheus 指标

`daemon` 模式会在 `0.0.0.0:9900/metrics` 暴露 Prometheus 指标。一次性执行任务不会启动该端点；Prometheus 应直接抓取 daemon 的 `9900` 端口。

宿主机安装 Prometheus 时，可将下面配置加入 Prometheus 配置文件，启动或重载后确认 target 状态为 `UP`：

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: mcim-rust-sync
    metrics_path: /metrics
    static_configs:
      - targets:
          - 127.0.0.1:9900
```

### 任务状态

以下指标的 `task` label 是 `schedule` 中的任务名：

| 指标 | 类型 | 含义 |
| --- | --- | --- |
| `mcim_sync_task_runs_total{task,result}` | Counter | 任务完成次数，`result` 为 `success`、`partial_failure` 或 `error` |
| `mcim_sync_task_duration_seconds{task}` | Histogram | 任务执行耗时 |
| `mcim_sync_task_running{task}` | Gauge | 当前是否正在运行，`1` 表示运行中 |
| `mcim_sync_task_failures_streak{task}` | Gauge | 连续整体失败次数，部分条目失败不计入连续失败 |
| `mcim_sync_task_overlap_skips_total{task}` | Counter | 因上一轮尚未结束而跳过的次数 |
| `mcim_sync_task_last_run_timestamp_seconds{task}` | Gauge | 最近一次任务完成时间，Unix 时间戳 |
| `mcim_sync_task_last_success_timestamp_seconds{task}` | Gauge | 最近一次完全成功完成时间，Unix 时间戳 |
| `mcim_sync_task_last_result{task,result}` | Gauge | 最近一次结果，当前结果对应的 `result` 值为 `1`，其余为 `0` |
| `mcim_sync_uptime_seconds` | Gauge | 进程运行时间 |

`task_duration_seconds` 是 Histogram，可使用 `_bucket`、`_sum` 和 `_count` 后缀查询分位数或平均耗时。

### 同步业务统计

`mcim_sync_items_total` 是按每次任务的 `TaskSummary` 累计的 Counter，label 如下：

```text
task       schedule 中的任务名
provider   modrinth 或 curseforge
entity     project、mod、version 或 file
operation  queue、refresh、refresh_full 或 search
result     attempted、synced、not_found、skipped、failed、requeued、discovered 或 removed
```

统计范围：

- `project` / `mod`：本轮尝试、成功、未找到、跳过、失败、重新入队、新发现和删除数量。
- Modrinth 的 `version`：实际同步的版本数量。
- `file`：实际同步的文件数量，包含 Modrinth 和 CurseForge。
- `discovered`：`search` 任务发现且尚未入库的 project/mod 数量。
- `removed`：refresh 任务确认上游已删除并从缓存移除的 project 数量。

例如，查看每个任务成功同步的条目：

```promql
sum by (task, provider, entity) (
  increase(mcim_sync_items_total{result="synced"}[1h])
)
```

查看每个搜索任务发现的新 project/mod：

```promql
sum by (task, provider, entity) (
  increase(mcim_sync_items_total{result="discovered"}[24h])
)
```

### 内存指标

内存指标在 daemon 启动时采样一次，之后每 5 秒更新：

| 指标 | 含义 |
| --- | --- |
| `mcim_sync_process_resident_memory_bytes` | 进程当前 RSS |
| `mcim_sync_process_virtual_memory_bytes` | 进程虚拟内存 |
| `mcim_sync_process_peak_resident_memory_bytes` | 进程峰值 RSS |
| `mcim_sync_cgroup_memory_current_bytes` | 当前 cgroup 内存占用 |
| `mcim_sync_cgroup_memory_peak_bytes` | cgroup 峰值内存占用 |
| `mcim_sync_cgroup_memory_limit_bytes` | cgroup 内存上限 |

例如，当前容器内存使用率：

```promql
mcim_sync_cgroup_memory_current_bytes
/
mcim_sync_cgroup_memory_limit_bytes
```

这些指标不记录 project ID、mod ID、URL 或错误文本，避免产生高基数时序。单次任务的详细状态和摘要应通过任务历史 API 查看。

## 任务历史 API

daemon 还会在 `TASK_API_ADDR`（默认 `0.0.0.0:9901`）提供任务运行历史接口。每次任务开始时先写入 `running` 记录，结束时更新为 `success`、`partial_failure` 或 `error`，并保存开始时间、结束时间、耗时和 `TaskSummary` 汇总；不会保存逐条同步日志、project/mod ID 或请求内容。进程重启时，遗留的 `running` 记录会标记为 `interrupted`。

记录保存在 MongoDB 的 `task_runs` 集合中，daemon 启动时会创建查询索引。

```text
GET /healthz
GET /api/task-runs?task=modrinth-refresh&status=success&from=1760000000000&to=1760100000000&limit=100
```

`/api/task-runs` 按 `started_at` 倒序返回记录。`task`、`status`、`from`、`to` 均可选，其中 `from`/`to` 是 Unix 毫秒时间戳，`limit` 范围为 1 到 500，响应格式为：

```json
{"data": [{"task":"modrinth-refresh","status":"success","duration_ms":1234,"total":100,"synced":20,"versions":35,"files":80}],"count":1}
```

Grafana 可使用 Infinity 等 JSON 数据源读取 `http://<sync-host>:9901/api/task-runs`，用 `task`、`status` 和 `limit` 查询参数制作任务明细表；Prometheus 继续用于趋势、告警和聚合指标。

## 鸣谢

谢谢来自 [HyacinthHaru](https://github.com/HyacinthHaru) 的支持！

之前有过尝试，最终在 HyacinthHaru 的支持下才重新开始。
