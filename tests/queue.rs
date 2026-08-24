//! 队列 key 的约定，必须与 mcim-rust-api 的写入端一致

use mcim_rust_sync::db::queue::key;

/// Modrinth 的 `/v2/version_files` 只支持 sha1 与 sha512
///
/// Python 版消费端遍历的是 sha1 与 sha256，清空端删的却是 sha1 与 sha512，
/// 结果 sha512 队列从来没被消费过却每轮被删掉
#[test]
fn hash_algorithms_match_queue_keys() {
    assert_eq!(key::MODRINTH_HASH_ALGORITHMS, ["sha1", "sha512"]);
    assert_eq!(key::modrinth_hashes("sha1"), "modrinth_hashes_sha1");
    assert_eq!(key::modrinth_hashes("sha512"), "modrinth_hashes_sha512");
}

#[test]
fn queue_key_names() {
    assert_eq!(key::CURSEFORGE_MODIDS, "curseforge_modids");
    assert_eq!(key::CURSEFORGE_FILEIDS, "curseforge_fileids");
    assert_eq!(key::CURSEFORGE_FINGERPRINTS, "curseforge_fingerprints");
    assert_eq!(key::MODRINTH_PROJECT_IDS, "modrinth_project_ids");
    assert_eq!(key::MODRINTH_VERSION_IDS, "modrinth_version_ids");
}
