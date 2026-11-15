//! 1c-connect: the developer's side of the environment. A tunnel into the
//! container with an embedded SSH client, and a bridge that starts mcp-1c on
//! the developer's base behind each forwarded connection. Nothing is
//! installed on this machine; the program lives on a flash drive.

pub mod allow;
pub mod analyzer;
pub mod bridge;
pub mod cfg;
pub mod channel;
/// Подробный журнал шагов рукопожатия — по переменной `DEV1C_DEBUG` (просьба владельца 15.11).
pub mod debug;
pub mod editors;
pub mod enroll;
pub mod enrolled;
pub mod flow;
pub mod gate;
#[cfg(feature = "gui")]
pub mod gui;
pub mod holder;
pub mod job;
pub mod jsonrpc;
pub mod licence;
pub mod listen;
pub mod local;
pub mod need;
pub mod node;
pub mod onec_error;
pub mod ops_host;
pub mod orphan;
pub mod peer;
pub mod peer_out;
pub mod platform;
pub mod ports;
pub mod project;
pub mod publication;
pub mod secrets;
pub mod session;
pub mod single_instance;
pub mod skills;
pub mod ticket;
pub mod verify;
pub mod wire;

/// Предмет, который включает выключатель, — одним словом на все поверхности: трей, карточка, журнал, подсказки, имена фактов, проверки
/// готовности (`verify`). Сокращения «MCP» на экране больше нет: у 1С-разработчика оно ещё отзовётся, у бухгалтера — нет (находка 12
/// осмотра 10.11). И это именно ПРЕДМЕТ, а не объяснение отказа: подпись у выключателя обязана называть то, что включается, а причина
/// живёт в подсказке и в журнале (находка 13).
///
/// **Почему в корне, а не в окне.** Это слово ПРОДУКТА, а не слово экрана: теми же знаками его пишет `verify` — код, который работает и
/// в сборке без окна. Пока константа жила в `gui`, бек повторял её литералом, и сходились две записи только потому, что пока совпадали.
/// Две меры на один вопрос — главный разряд дефектов этого дерева; здесь мера одна, и видят её обе стороны.
///
/// Сокращение остаётся во внутренних именах кода (`EDITOR_MCP`, `mcp_exe`, `mcp-1c`) — там оно точнее любого перевода и человеку не
/// показывается.
pub const SUBJECT: &str = "доступ к 1С";
