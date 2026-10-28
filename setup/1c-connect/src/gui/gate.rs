//! Холодный замок переехал в `crate::gate` — он не за фичей `gui`: условия не
//! зависят от окна, а подкоманде `1c-connect mcp` (`ops_host::run_mcp`) нужны
//! ровно те же, без второй формулировки причин отказа. Здесь — реэкспорт, чтобы
//! `gui::gate::cold_gate`/`gui::gate::wants_directory` остались тем же путём,
//! каким их называет остальной код окна.
pub use crate::gate::{cold_gate, wants_directory};
