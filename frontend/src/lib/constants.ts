// 跨层契约常量集中处：这些值在前后端两侧必须逐字一致，散落硬编码时
// 改动一侧不会报错，只会静默失效，故集中定义并注明来源。
//
// 新增常量请一并写清「另一端在哪」，便于改动时同步。

/** 后端 `crate::crypto::SECRET_MASK`（src-tauri/src/crypto.rs） */
export const SECRET_MASK = "·";

/** 后端 `AiConfig.api_key_env` 的固定回落值；界面不暴露此项（P18-6），
 *  仅用于满足配置结构（真正取 Key 的优先级：api_key 密文 > 环境变量） */
export const DEFAULT_API_KEY_ENV = "API_KEY";
