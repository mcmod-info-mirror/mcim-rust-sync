/// 432 是 Minecraft，78022 是 Minecraft Bedrock Edition
pub const ACCEPT_GAME_IDS: [i32; 2] = [432, 78022];

/// 搜索发现按 class 分别翻页，这里是各游戏下的 classId
pub const GAME_432_CLASS_IDS: [i32; 9] = [4546, 4559, 12, 5, 6, 4471, 17, 6552, 6945];
pub const GAME_78022_CLASS_IDS: [i32; 5] = [4984, 6913, 6929, 6940, 6925];

pub fn class_ids(game_id: i32) -> &'static [i32] {
    match game_id {
        78022 => &GAME_78022_CLASS_IDS,
        _ => &GAME_432_CLASS_IDS,
    }
}

/// CurseForge 深分页上限
pub const CURSEFORGE_SEARCH_LIMIT: i64 = 10000;
pub const CURSEFORGE_SEARCH_PAGE_SIZE: i64 = 50;

/// 一次性拉完某个 mod 的文件列表，失败时退回逐页
pub const CURSEFORGE_FILES_PAGE_SIZE: i64 = 10000;
pub const CURSEFORGE_FILES_FALLBACK_PAGE_SIZE: i64 = 50;

pub const MODRINTH_SEARCH_PAGE_SIZE: i64 = 100;
