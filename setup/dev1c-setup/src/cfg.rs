//! Параметры комплекта: `dev1c.toml`. Единственный источник; переменные
//! контейнера, порты и имена томов выводятся отсюда.

use anyhow::{Context, Result};
use common::kit::Kit;
use serde::{Deserialize, Serialize};

/// Образец с комментариями: он же кладётся в комплект как dev1c.toml. Тест
/// ниже держит его и структуру в согласии.
pub const EXAMPLE: &str = include_str!("../dev1c.example.toml");

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cfg {
    pub server: Server,
    pub model: Model,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub name: String,
    pub port: u16,
    pub memory: String,
    pub gpu: bool,
    pub engine: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Model {
    /// "host" — служба этой машины, "docker" — контейнер рядом, "remote" — чужой сервер.
    pub serve: String,
    pub file: String,
    pub url: String,
    pub key: String,
    pub name: String,
    /// Порт службы хоста (на петле). Для docker наружу идёт третий порт среды.
    pub port: u16,
    /// Предел памяти контейнера модели («22g»). Пусто — посчитать от машины.
    pub memory: String,
    /// На сколько агентов делить окно. 1 — не делить (см. спеку: окно на три не делим).
    pub agents: u16,
    /// Мест (слотов) у сервера модели: по одному на рабочее место. Меньше
    /// трёх — когда карта не вмещает три окна рядом с весами.
    pub slots: u16,
    /// Нижняя граница окна НА СЛОТ (`--fit-ctx`): само окно подбирает сервер
    /// под карту и обычно берёт больше. Не держит и минимума — сервер не
    /// стартует; дал меньше заказанного — установка проваливается с обоими
    /// числами.
    pub fit_ctx: u32,
}

impl Default for Server {
    fn default() -> Self {
        Self { name: "dev1c".into(), port: 3390, memory: "24g".into(), gpu: false, engine: "docker".into() }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            serve: "host".into(),
            file: String::new(),
            url: String::new(),
            key: String::new(),
            name: String::new(),
            port: 1235,
            memory: String::new(),
            agents: 1,
            slots: crate::llama::SLOTS,
            fit_ctx: 81920,
        }
    }
}

/// Форма сервера модели.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form {
    Remote,
    Host,
    Docker,
}

/// Что установщик измерил у поднятого сервера; без этого контейнеру среды
/// нечего сказать про локальную модель.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// Окно одного слота, как его сообщил сервер. Чисел для Zed здесь больше
    /// нет: штатный провайдер `llama.cpp` забирает окно с `/props` сам
    /// (решение владельца 2026-10-21).
    pub window: u32,
}

pub const FILE: &str = "dev1c.toml";

/// Полный ли путь — на любой из двух систем: комплект пишется на Windows,
/// а читается и там, и на Linux. Правило одно у проверки параметров и у
/// сборки образца, поэтому живёт в одном месте, а не в замыкании внутри
/// validate.
fn absolute(p: &str) -> bool {
    p.starts_with('/') || p.starts_with('\\') || p.get(1..3).is_some_and(|s| s == ":\\" || s == ":/")
}

impl Cfg {
    /// Читает `dev1c.toml` из комплекта. Нет файла — значения по умолчанию,
    /// и `status` скажет об этом.
    pub fn load(kit: &Kit) -> Result<Cfg> {
        let p = kit.path(FILE);
        if !p.exists() {
            return Ok(Cfg::default());
        }
        let text = std::fs::read_to_string(&p).with_context(|| format!("не прочитать {}", p.display()))?;
        let cfg: Cfg = toml::from_str(&text).with_context(|| format!("{FILE}: ошибка в файле параметров"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn form(&self) -> Form {
        match self.model.serve.as_str() {
            "remote" => Form::Remote,
            "docker" => Form::Docker,
            _ => Form::Host,
        }
    }

    /// Имя модели для Zed: name, а без него — имя файла без расширения.
    pub fn model_alias(&self) -> String {
        if !self.model.name.is_empty() {
            return self.model.name.clone();
        }
        // Path::file_stem on Linux does not split a Windows path: do it by hand.
        let tail = self.model.file.rsplit(['/', '\\']).next().unwrap_or_default();
        tail.rsplit_once('.').map(|(s, _)| s).unwrap_or(tail).to_string()
    }

    /// Адрес сервера модели, как его видит контейнер среды. С `/v1` — это то,
    /// что ждёт провайдер `openai_compatible`: путь до, но не включая
    /// `/chat/completions`, который Zed достраивает сам.
    pub fn model_url(&self) -> String {
        match self.form() {
            Form::Remote => self.model.url.clone(),
            Form::Host => format!("http://host.docker.internal:{}/v1", self.model.port),
            Form::Docker => format!("http://host.docker.internal:{}/v1", self.ports()[2]),
        }
    }

    /// Тот же сервер, но корнем — без `/v1`. Штатному провайдеру `llama.cpp`
    /// нужен именно корень: он сам ходит в `/props` за окном и в `/v1/models`
    /// за моделями, и адрес с `/v1` превратил бы это в `/v1/v1/...`.
    pub fn model_root(&self) -> String {
        let u = self.model_url();
        u.strip_suffix("/v1").unwrap_or(&u).to_string()
    }

    /// Ошибка называет поле и что с ним делать.
    pub fn validate(&self) -> Result<()> {
        let m = &self.model;
        anyhow::ensure!(matches!(m.serve.as_str(), "host" | "docker" | "remote"), "[model]: serve — host, docker или remote");
        anyhow::ensure!((1..=3).contains(&m.agents), "[model]: agents — от 1 до 3");
        // Больше трёх мест бессмысленно: рабочих мест в стенде три, и
        // лишний слот только отрезает окно у остальных.
        anyhow::ensure!((1..=3).contains(&m.slots), "[model]: slots — от 1 до 3 (по одному на рабочее место)");
        // Окно обязано вмещать вход не меньше MIN_BUDGET и место под ответ:
        // иначе параметр не отказал бы здесь, а провалил установку после
        // замера, когда сервер уже поднят.
        // На agents умножаем: окно слота делится на агентов ПЕРВЫМ, и при
        // agents = 3 окно 28672 давало бы 9557 на агента — на вход не осталось
        // бы ничего, и отказала бы установка после замера, а не этот параметр.
        // Запаса Zed в сумме больше нет: со штатным провайдером окно приходит
        // с `/props`, и редактор делит его сам, а не по нашему объявлению.
        let floor = (crate::llama::MIN_BUDGET + crate::llama::ANSWER_FLOOR) * m.agents.max(1) as u32;
        anyhow::ensure!(
            m.fit_ctx >= floor,
            "[model]: fit_ctx — не меньше {floor} (вход {} + ответ {}); при меньшем окне на 1С не хватит даже одного модуля",
            crate::llama::MIN_BUDGET,
            crate::llama::ANSWER_FLOOR
        );
        // Предел памяти, который администратор написал, а мы молча отбросили,
        // хуже отказа: "22" — это 22 байта, "22gb" не читается вовсе.
        if !m.memory.is_empty() {
            let unit = matches!(m.memory.chars().last(), Some('g' | 'G' | 'm' | 'M'));
            let digits = unit && m.memory[..m.memory.len() - 1].trim().parse::<f64>().is_ok();
            let big_enough = digits && crate::llama::parse_size_mb(&m.memory).is_some_and(|mb| mb >= crate::llama::MIN_MODEL_MB as u64);
            anyhow::ensure!(
                big_enough,
                "[model]: memory — предел памяти контейнера модели с единицей: \"22g\" или \"22528m\", не меньше {} МиБ; пусто — посчитать от машины",
                crate::llama::MIN_MODEL_MB
            );
        }
        anyhow::ensure!(
            m.file.is_empty() || m.url.is_empty(),
            "[model]: задано и file, и url — оставьте одно: файл (serve = \"host\" или \"docker\") или удалённый сервер (serve = \"remote\")"
        );
        match self.form() {
            Form::Remote => {
                anyhow::ensure!(!m.url.is_empty(), "[model]: для serve = \"remote\" нужен url");
                anyhow::ensure!(!m.name.is_empty(), "[model]: для удалённого сервера нужно name — имя модели, как сервер его называет");
                // key may be empty: a server inside the network often has none.
            }
            Form::Host => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\" — выберите одно: serve = \"remote\" или пустой url");
                anyhow::ensure!(
                    m.file.is_empty() || absolute(&m.file),
                    "[model]: при serve = \"host\" file — полный путь к .gguf на этой машине; для файла в комплекте выберите serve = \"docker\""
                );
            }
            Form::Docker => {
                anyhow::ensure!(m.url.is_empty(), "[model]: url задан, а serve не \"remote\" — выберите одно: serve = \"remote\" или пустой url");
                anyhow::ensure!(m.file.is_empty() || !absolute(&m.file), "[model]: при serve = \"docker\" file — путь относительно комплекта");
            }
        }
        anyhow::ensure!(
            self.server.port >= 1024 && self.server.port <= 65532,
            "[server]: port должен быть от 1024 до 65532 (нужны четыре порта подряд)"
        );
        anyhow::ensure!(matches!(self.server.engine.as_str(), "docker" | "podman"), "[server]: engine — docker или podman");
        Ok(())
    }

    /// RDP, Gitea, модель, SSH.
    pub fn ports(&self) -> [u16; 4] {
        let p = self.server.port;
        [p, p + 1, p + 2, p + 3]
    }

    /// Что уходит в контейнер через -e. Локальные формы говорят про модель
    /// только после замера (`facts`): адрес, токен и имя.
    ///
    /// Провайдер называется здесь, а не в настройках: их один файл на все
    /// рабочие места, и выбирать ветку в нём некому. Наш сервер — штатный
    /// `llama.cpp`, он забирает окно с `/props` сам, поэтому чисел для Zed мы
    /// больше не передаём. Чужой OpenAI-совместимый сервер (`serve =
    /// "remote"`) штатный провайдер не понимает, и для него остаётся
    /// `openai_compatible.custom` — у него `/props` нет, окно объявляется
    /// умолчаниями entrypoint.
    ///
    /// Ключа у НАШЕГО сервера нет вовсе (решение 26.10.2026): порт он
    /// публикует на мосту, то есть снаружи машины недоступен, а закрывал
    /// только тех, кто уже на ней. Взамен ключ отнимал веб-чат llama.cpp —
    /// его интерфейс посылать ключ не умеет, и каждый запрос получал 401.
    /// У чужого сервера (`serve = "remote"`) ключ остаётся: он не наш, и
    /// имя переменной там диктует Zed — `CUSTOM_API_KEY`.
    pub fn container_env(&self, facts: Option<&Facts>) -> Vec<(String, String)> {
        let m = &self.model;
        let mut env = Vec::new();
        match self.form() {
            Form::Remote => {
                env.push(("MODEL_PROVIDER".into(), "custom".into()));
                env.push(("CUSTOM_API_URL".into(), m.url.clone()));
                if !m.key.is_empty() {
                    env.push(("CUSTOM_API_KEY".into(), m.key.clone()));
                }
                env.push(("MODEL_NAME".into(), m.name.clone()));
            }
            Form::Host | Form::Docker => {
                // Факты нужны самим по себе: без замера контейнер не
                // собирают. Из них в окружение сегодня не уходит ничего —
                // окно редактор берёт у сервера сам.
                let Some(_) = facts else { return env };
                env.push(("MODEL_PROVIDER".into(), "llama.cpp".into()));
                env.push(("LLAMA_API_URL".into(), self.model_root()));
                env.push(("MODEL_NAME".into(), self.model_alias()));
            }
        }
        env
    }

    /// `dev1c.toml` с текущими значениями и всеми комментариями образца:
    /// `change` правит значения на месте, а не пересобирает файл с нуля.
    pub fn render(cfg: &Cfg) -> String {
        common::toml_render::render(EXAMPLE, |section, key| {
            Some(match (section, key) {
                ("[server]", "name") => format!("{:?}", cfg.server.name),
                ("[server]", "port") => cfg.server.port.to_string(),
                ("[server]", "memory") => format!("{:?}", cfg.server.memory),
                ("[server]", "gpu") => cfg.server.gpu.to_string(),
                ("[server]", "engine") => format!("{:?}", cfg.server.engine),
                ("[model]", "serve") => format!("{:?}", cfg.model.serve),
                ("[model]", "file") => format!("{:?}", cfg.model.file),
                ("[model]", "url") => format!("{:?}", cfg.model.url),
                ("[model]", "key") => format!("{:?}", cfg.model.key),
                ("[model]", "name") => format!("{:?}", cfg.model.name),
                ("[model]", "port") => cfg.model.port.to_string(),
                ("[model]", "memory") => format!("{:?}", cfg.model.memory),
                ("[model]", "agents") => cfg.model.agents.to_string(),
                ("[model]", "slots") => cfg.model.slots.to_string(),
                ("[model]", "fit_ctx") => cfg.model.fit_ctx.to_string(),
                _ => return None,
            })
        })
    }

    /// Образец с одной подставленной формой обслуживания: умалчивание
    /// (`Cfg::default()`) не трогаем — только то, что нужно показать.
    ///
    /// `model` — файл модели, если он на эту сборку комплекта известен. Имя
    /// модели нарочно не зашито в программу: модель пилота меняется чаще, чем
    /// установщик, и зашитое имя пришлось бы править кодом и пересобирать.
    /// Задаётся снаружи — `example --model`, а в сборке комплекта
    /// DEV1C_MODEL_FILE (см. pack.sh). Не задано — `file` остаётся пустым, и
    /// установка говорит «модель не выбрана», как и прежде.
    ///
    /// Путь проверяется здесь же: для `docker` он обязан быть относительным
    /// (файл лежит в комплекте), для `host` — полным. Иначе ошибка вышла бы у
    /// заказчика при установке, а не у нас при сборке.
    pub fn example_for(serve: &str, model: Option<&str>) -> Result<String> {
        let mut cfg = Cfg::default();
        cfg.model.serve = serve.into();
        if let Some(m) = model {
            // Проверяем ровно заданное. Весь файл проверять нельзя: образец
            // нарочно неполон — у формы remote в нём пустые url и name, их
            // заполняет администратор, и validate() на образце упал бы.
            match cfg.form() {
                Form::Docker => anyhow::ensure!(
                    !absolute(m),
                    "--model при serve = \"docker\" — путь относительно комплекта (model/<файл>.gguf), а не полный: {m}"
                ),
                Form::Host => anyhow::ensure!(
                    absolute(m),
                    "--model при serve = \"host\" — полный путь к .gguf на этой машине, а не относительный: {m}"
                ),
                Form::Remote => anyhow::bail!(
                    "--model с serve = \"remote\" не нужен: у удалённого сервера нет файла, есть url и name"
                ),
            }
            cfg.model.file = m.into();
        }
        // При serve = "docker" сервер модели живёт на этой же машине и просит
        // памяти больше, чем среда: раздел стенда из спеки — 8 ГБ среде,
        // остальное (на 32 ГБ это 22 ГБ) контейнеру модели. Умолчание 24g
        // верно для рабочей станции, где модель отдана службе хоста, но
        // оставлять его здесь значит отдать модели 6 ГБ и не сказать об этом.
        if cfg.form() == Form::Docker {
            cfg.server.memory = "8g".into();
        }
        Ok(Cfg::render(&cfg))
    }

    pub fn volume(&self, suffix: &str) -> String {
        format!("{}-{}", self.server.name, suffix)
    }

    /// Образ, из которого поднимается контейнер. Это СТЕНД, а не среда:
    /// oktaplus/dev1c-1c собран поверх oktaplus/dev1c и несёт 1С, vrunner,
    /// коннектор и xauth. Пока здесь стояла среда, заказчик по комплекту
    /// получал контейнер без 1С, в который вдобавок нельзя войти по RDP —
    /// без xauth сессия не создаёт ключ, и xrdp говорит «X server could not
    /// be started».
    ///
    /// Образ в комплекте ОДИН, и это он. Основа (oktaplus/dev1c) нужна там,
    /// где стенд собирают: дистрибутив 1С весит 1.7 ГБ и выдаётся с сайта 1С
    /// через браузерную форму, на runner его не положить, поэтому среда
    /// собирается в Actions, а стенд — слоем поверх, где лежит файл
    /// (Dockerfile.1c). Заказчику основа не нужна ничем: контейнер
    /// поднимается из стенда, состояние проверяется по стенду, собирать у
    /// заказчика нечего. Пока её тоже везли, у него оставался второй тег,
    /// которым никто не пользовался, и `clean --data` снимал тот тег только
    /// потому, что мы сами же его и привезли.
    pub fn image(&self) -> String {
        "oktaplus/dev1c-1c:latest".into()
    }

    /// Образ сервера модели (serve = "docker"); едет своим архивом
    /// (dev1c-llama-image.tar.gz): у него другой базовый образ, общих слоёв
    /// со стендом нет, и в одном файле они не экономят ничего.
    pub fn llama_image(&self) -> String {
        "oktaplus/dev1c-llama:latest".into()
    }

    pub fn llama_container(&self) -> String {
        format!("{}-llama", self.server.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_parses_and_equals_defaults() {
        let c: Cfg = toml::from_str(EXAMPLE).expect("example toml parses");
        assert_eq!(c, Cfg::default(), "dev1c.example.toml and Cfg::default drifted apart");
    }

    #[test]
    fn ports_follow_the_first_one() {
        let mut c = Cfg::default();
        c.server.port = 4000;
        assert_eq!(c.ports(), [4000, 4001, 4002, 4003]);
    }

    #[test]
    fn remote_model_gives_custom_variables_and_no_llama() {
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m.example.com/v1".into();
        c.model.key = "k".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env(None);
        assert!(env.contains(&("CUSTOM_API_URL".into(), "https://m.example.com/v1".into())));
        assert!(env.contains(&("CUSTOM_API_KEY".into(), "k".into())));
        assert!(env.contains(&("MODEL_NAME".into(), "Q".into())));
        assert!(env.iter().all(|(k, _)| k != "LLAMA_ENABLED"), "внутренней лламы больше нет");
    }

    #[test]
    fn default_form_is_host_and_needs_an_absolute_path() {
        let mut c = Cfg::default();
        assert_eq!(c.form(), Form::Host);
        c.model.file = "model/x.gguf".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("serve = \"docker\""), "{e}");
        c.model.file = if cfg!(windows) { r"C:\models\x.gguf".into() } else { "/models/x.gguf".into() };
        c.validate().unwrap();
        assert_eq!(c.model_alias(), "x");
        assert_eq!(c.model_url(), "http://host.docker.internal:1235/v1");
    }

    #[test]
    fn docker_form_takes_the_file_from_the_kit_and_the_third_port() {
        let mut c = Cfg::default();
        c.model.serve = "docker".into();
        c.model.file = "model/x.gguf".into();
        c.validate().unwrap();
        assert_eq!(c.form(), Form::Docker);
        assert_eq!(c.model_url(), "http://host.docker.internal:3392/v1");
    }

    #[test]
    fn remote_form_is_chosen_by_serve_not_guessed() {
        let mut c = Cfg::default();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("serve"), "{e}");
        c.model.serve = "remote".into();
        c.validate().unwrap();
        assert_eq!(c.model_url(), "https://m");
    }

    #[test]
    fn local_forms_send_the_measured_numbers_and_nothing_before_them() {
        let mut c = Cfg::default();
        c.model.file = if cfg!(windows) { r"C:\models\x.gguf".into() } else { "/models/x.gguf".into() };
        // До замера фактов нет: контейнеру нечего сказать про модель.
        let env = c.container_env(None);
        assert!(env.is_empty(), "{env:?}");
        // Локальный сервер — штатный провайдер Zed: адрес КОРНЕМ, без /v1,
        // ключ переменной, имя которой диктует Zed, и ни одного числа окна —
        // его провайдер забирает с /props сам.
        let f = Facts { window: 250_000 };
        let env = c.container_env(Some(&f));
        assert!(env.contains(&("MODEL_PROVIDER".into(), "llama.cpp".into())), "{env:?}");
        assert!(env.contains(&("LLAMA_API_URL".into(), "http://host.docker.internal:1235".into())), "{env:?}");
        // Ключа у нашего сервера нет вовсе (решение 26.10.2026): он
        // закрывал только тех, кто уже на сервере, а взамен отнимал
        // веб-чат llama.cpp — его интерфейс ключ посылать не умеет.
        assert!(!env.iter().any(|(k, _)| k == "LLAMACPP_API_KEY"), "{env:?}");
        assert!(env.contains(&("MODEL_NAME".into(), "x".into())), "{env:?}");
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !names.iter().any(|k| k.starts_with("CUSTOM_MAX")),
            "числа окна в переменных больше не передаются: {names:?}"
        );
    }

    #[test]
    fn agents_and_fit_ctx_have_sane_bounds() {
        let mut c = Cfg::default();
        c.model.agents = 0;
        assert!(c.validate().unwrap_err().to_string().contains("agents"));
        c.model.agents = 1;
        c.model.fit_ctx = 1000;
        assert!(c.validate().unwrap_err().to_string().contains("fit_ctx"));
        // 24576 раньше проходило, а теперь нет: в такое окно не влезает даже
        // старт агента (32 000 токенов правил, скиллов и схем инструментов —
        // см. MIN_BUDGET). Пусть отказывает конфиг, а не замер на поднятом
        // сервере. Пол — вход плюс ответ, запаса Zed в нём больше нет.
        c.model.fit_ctx = 57344;
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("81920"), "{e}");
        c.model.fit_ctx = 81920;
        assert!(c.validate().is_ok(), "{:?}", c.validate());
        // Деление на агентов учитывается: то же окно на трёх агентов не годится.
        c.model.agents = 3;
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("245760"), "{e}");
        // Три агента на одном слоте требуют трёх стартов и трёх рабочих
        // мест в окне — вот столько это и стоит.
        c.model.fit_ctx = 245_760;
        assert!(c.validate().is_ok(), "{:?}", c.validate());
    }

    #[test]
    fn a_memory_limit_we_would_not_understand_is_refused_not_ignored() {
        let mut c = Cfg::default();
        for bad in ["22", "22gb", "g", "2g"] {
            c.model.memory = bad.into();
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains("memory") && e.contains("22g"), "{bad}: {e}");
        }
        for good in ["22g", "22528m", "8G"] {
            c.model.memory = good.into();
            c.validate().unwrap();
        }
        c.model.memory = String::new();
        c.validate().unwrap();
    }

    #[test]
    fn both_file_and_url_is_refused_with_the_field_named() {
        let mut c = Cfg::default();
        c.model.file = "model/x.gguf".into();
        c.model.url = "https://m".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("[model]"), "{e}");
    }

    #[test]
    fn url_without_key_is_allowed_and_sends_no_key() {
        // A server inside the network (llama-server, vLLM) has no key at all.
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        c.validate().unwrap();
        let env = c.container_env(None);
        assert!(env.iter().all(|(k, _)| k != "CUSTOM_API_KEY"));
        assert!(env.contains(&("MODEL_NAME".into(), "Q".into())));
    }

    #[test]
    fn change_keeps_the_comments() {
        let mut c = Cfg::default();
        assert_eq!(Cfg::render(&c), EXAMPLE, "defaults render to the example itself");
        c.server.port = 4000;
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        c.model.name = "Q".into();
        c.model.agents = 3;
        let t = Cfg::render(&c);
        assert!(t.contains("port = 4000"));
        assert!(t.contains("serve = \"remote\""));
        assert!(t.contains("url = \"https://m\""));
        assert!(t.contains("agents = 3"));
        // `name` exists in both sections: only [model] name changes.
        assert!(t.contains("name = \"dev1c\"") && t.contains("name = \"Q\""));
        assert!(t.contains("# Первый из четырёх портов подряд"));
        let back: Cfg = toml::from_str(&t).unwrap();
        assert_eq!(back, c);
    }

    /// Файл модели в образце — снаружи, а не зашитый: имя модели пилота
    /// меняется чаще установщика. Путь проверяется при сборке комплекта, а не
    /// при установке у заказчика.
    #[test]
    fn the_sample_carries_the_model_file_only_when_it_is_given() {
        let kit = Cfg::example_for("docker", Some("model/GLM-4.7-Flash-UD-Q4_K_XL.gguf")).unwrap();
        assert!(kit.contains("file = \"model/GLM-4.7-Flash-UD-Q4_K_XL.gguf\""), "{kit}");
        // Не задано — как было: пусто, и установка скажет «модель не выбрана».
        assert!(Cfg::example_for("docker", None).unwrap().contains("file = \"\""));
        // Форма docker ждёт путь В КОМПЛЕКТЕ: полный путь — ошибка сборки.
        assert!(Cfg::example_for("docker", Some("/srv/models/x.gguf")).is_err());
        // Форма host — наоборот, полный путь на той машине.
        assert!(Cfg::example_for("host", Some("model/x.gguf")).is_err());
        assert!(Cfg::example_for("host", Some("/srv/models/x.gguf")).unwrap().contains("/srv/models/x.gguf"));
        // Удалённому серверу файл не нужен вовсе.
        assert!(Cfg::example_for("remote", Some("/srv/models/x.gguf")).is_err());
    }

    #[test]
    fn the_sample_can_be_printed_for_any_form() {
        // Форма docker делит память по спеке стенда: среде 8 ГБ, остальное
        // модели. Остальные формы отдают память среде, как и раньше.
        assert!(Cfg::example_for("docker", None).unwrap().contains("memory = \"8g\""));
        assert!(Cfg::example_for("host", None).unwrap().contains("memory = \"24g\""));
        // Форма host не подменяет ничего — значит это сам dev1c.example.toml,
        // байт в байт. Держалось это только косвенно (через «memory = 24g» и
        // равенство EXAMPLE == Cfg::default() в тестах выше), а правка
        // example_for могла незаметно развести образец комплекта с файлом.
        assert_eq!(Cfg::example_for("host", None).unwrap(), EXAMPLE, "образец для host — это и есть dev1c.example.toml");
        assert_ne!(Cfg::example_for("docker", None).unwrap(), EXAMPLE, "форма docker обязана подменить память контейнера среды");
        assert_eq!(Cfg::default().server.memory, "24g", "умолчание не тронуто");
        for form in ["host", "docker", "remote"] {
            let text = Cfg::example_for(form, None).unwrap();
            assert!(text.contains(&format!("serve = \"{form}\"")));
            // Комментарии образца на месте: файл остаётся человеческим.
            assert!(text.contains('#'));
            // И он читается обратно без потерь.
            let back: Cfg = toml::from_str(&text).unwrap();
            assert_eq!(back.model.serve, form);
        }
    }

    #[test]
    fn url_without_name_is_refused() {
        let mut c = Cfg::default();
        c.model.serve = "remote".into();
        c.model.url = "https://m".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("name"), "{e}");
    }

    #[test]
    fn empty_model_is_allowed() {
        // Пустая модель — допустимо (файл зальют позже), агент просто молчит.
        assert!(Cfg::default().validate().is_ok());
    }

    #[test]
    fn volumes_use_the_container_name() {
        let c = Cfg::default();
        assert_eq!(c.volume("home"), "dev1c-home");
    }
}
