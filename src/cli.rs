use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::constants::ACCEPT_GAME_IDS;

#[derive(Debug, Parser)]
#[command(
    name = "mcim-rust-sync",
    version,
    about = "拉取 CurseForge 与 Modrinth 的信息写入 MCIM 缓存库",
    long_about = "每次运行只执行一个任务，跑完即退出，调度交给 cron 或 systemd timer"
)]
pub struct Cli {
    /// 配置文件路径
    #[arg(short, long, default_value = "config.json", global = true)]
    pub config: PathBuf,

    /// 输出 debug 级别日志
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// CurseForge 相关任务
    #[command(subcommand)]
    Curseforge(CurseforgeTask),

    /// Modrinth 相关任务
    #[command(subcommand)]
    Modrinth(ModrinthTask),

    /// 建立同步与查询所需的 MongoDB 索引，可重复执行
    Indexes,
}

#[derive(Debug, Subcommand)]
pub enum CurseforgeTask {
    /// 消费 Redis 队列里未命中的 modid、fileid 与 fingerprint
    Queue,

    /// 比对 dateModified，只同步有更新的 mod
    Refresh,

    /// 按发布时间倒序翻页，发现新收录的 mod
    Search {
        /// 只跑指定的 gameId，缺省时两个都跑
        #[arg(long)]
        game_id: Option<i32>,

        /// 每个 class 最多翻多少页，0 表示翻到上游给不出结果为止
        #[arg(long, default_value_t = 0)]
        max_pages: i64,

        /// 冷启动用：遇到已入库的条目也继续翻，直到上游没有更多结果
        #[arg(long)]
        full: bool,
    },

    /// 刷新分类
    Categories {
        #[arg(long)]
        game_id: Option<i32>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModrinthTask {
    /// 消费 Redis 队列里未命中的 project_id、version_id 与 hash
    Queue,

    /// 比对 updated 与版本列表，只同步有更新的项目
    Refresh,

    /// 重新同步库内全部项目
    RefreshFull,

    /// 按最新发布翻页，发现新收录的项目
    Search {
        /// 最多翻多少页，0 表示翻到上游给不出结果为止
        #[arg(long, default_value_t = 0)]
        max_pages: i64,

        /// 冷启动用：遇到已入库的条目也继续翻，直到上游没有更多结果
        #[arg(long)]
        full: bool,
    },

    /// 刷新 categories、loaders 与 game_versions
    Tags,
}

/// 未指定 gameId 时要覆盖的全部游戏
pub fn game_ids(explicit: Option<i32>) -> Vec<i32> {
    match explicit {
        Some(game_id) => vec![game_id],
        None => ACCEPT_GAME_IDS.to_vec(),
    }
}
