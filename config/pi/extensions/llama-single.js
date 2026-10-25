// Одномодельный llama.cpp как провайдер модели для pi.
//
// Зачем вообще: родной провайдер pi умеет только router-режим. Он берёт список
// через `GET /models` и ждёт поля `status.value`, которое появляется лишь в
// ответе роутера. Наш сервер поднят одной моделью (`-m`, `--parallel`,
// `--fit-ctx`), и в этом режиме такого поля нет — зато есть всё остальное:
//
//   {"object":"list","data":[{"id":"GLM-4.7-Flash-UD-Q4_K_XL","owned_by":"llamacpp",
//     "meta":{"n_ctx":161024,"n_ctx_train":202752}}]}
//
// То есть окно УЖЕ в ответе, в том самом поле `meta.n_ctx`, которое читает и
// pi. Поэтому расширение делает ровно одно: регистрирует провайдера с именем и
// окном, прочитанными у сервера.
//
// Окно нигде не объявляется числом, и это обязательное свойство: объявленное
// число — снимок на момент установки, а сервер пересчитывает окно при каждом
// старте от свободной видеопамяти. Разошлось — агент тихо глупеет. От этого
// уже уходили в настройках Zed, повторять нельзя.
//
// Когда одномодельный ответ `/models` получит поле `status` в самом llama.cpp
// (это полезно всем, и согласованность режимов авторы считают своей задачей),
// расширение выбрасывается, остаётся родной провайдер.

// Откуда берутся адрес и ключ. Переменные кладёт в /etc/environment точка
// входа, и ровно эти же читает редактор:
//   llama.cpp — наш сервер (контейнер рядом или служба на хосте),
//   custom    — «удалённый», то есть не нами поставленный.
// Для расширения разницы нет: лишь бы это был llama.cpp, у него `/props` и
// `meta.n_ctx` есть всегда.
//
// Имена переменных свои, а не LLAMA_BASE_URL/LLAMA_API_KEY: те читает родной
// провайдер, и заводить их значило бы обещать router-режим.
//
// Адрес и ключ — ПАРА, и берутся они вместе. Порознь получалось так: стенд
// удалённый (CUSTOM_*), а LLAMACPP_API_KEY остался в окружении от прежнего
// локального сервера — и к адресу custom уезжал ключ llamacpp, сервер отвечал
// 401, и у pi модели не было там, где у редактора она есть.
const SOURCES = [
    { provider: "llama.cpp", url: "LLAMA_API_URL", key: "LLAMACPP_API_KEY" },
    { provider: "custom", url: "CUSTOM_API_URL", key: "CUSTOM_API_KEY" },
];

// Предел ответа. Окно у сервера общее на вход и выход, а это — сколько из него
// разрешено занять ответу. 32768 — то же число, что у провайдера custom в
// настройках Zed и в dev1c-setup (ANSWER_FLOOR): модель размышляет, а
// размышления сидят в выходном бюджете, и на скупом пределе ответ приходит
// пустым. `Math.min` оставлен ради сервера с маленьким окном: предел ответа
// больше самого окна — бессмыслица.
const ANSWER_LIMIT = 32768;

// Сколько ждать сервер. Расширение работает до запуска агента, поэтому висеть
// нельзя: адрес может указывать на машину, которой нет, и тогда pi обязан
// запуститься без модели, а не ждать.
const TIMEOUT_MS = 5000;

const env = (name) => (process.env[name] || "").trim();

const getJson = async (url, headers) => {
    const res = await fetch(url, { headers, signal: AbortSignal.timeout(TIMEOUT_MS) });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
};

// Порядок проб. MODEL_PROVIDER — та же переменная, по которой точка входа
// решает, какой провайдер живой в настройках Zed; названный ею пробуется
// первым. Второй адрес при этом не выбрасывается: заданы оба, а первый молчит
// — модель берётся со второго, иначе у pi её нет там, где у редактора есть.
const candidates = () => {
    const live = env("MODEL_PROVIDER");
    const order = [...SOURCES].sort((a, b) => (b.provider === live) - (a.provider === live));
    return order
        .map((s) => ({ provider: s.provider, url: env(s.url), key: env(s.key) }))
        .filter((c) => c.url);
};

// Один адрес: что сервер отдал, то и объявляем. Ничего не получилось —
// возвращаем null и пробуем следующий.
const ask = async (c) => {
    // Завершающий `/v1` снимается: родной клиент pi поступает так же, а в
    // dev1c.toml адрес встречается в обеих формах.
    const base = c.url.replace(/\/+$/, "").replace(/\/v1$/, "");
    const headers = c.key ? { Authorization: `Bearer ${c.key}` } : {};

    let models;
    try {
        models = await getJson(`${base}/models`, headers);
    } catch (e) {
        // 401 здесь — почти всегда ключ не от этого адреса, и сказать это
        // надо словами: иначе «модели нет» выглядит как погашенный сервер.
        console.error(`llama-single: ${base} (${c.provider}) не ответил на /models: ${e.message}`);
        return null;
    }

    const model = models?.data?.[0];
    if (!model?.id) {
        console.error(`llama-single: ${base} (${c.provider}) отдал пустой список моделей`);
        return null;
    }
    const window = model.meta?.n_ctx;
    // Нет `meta.n_ctx` — это не llama.cpp, а чужой шлюз. Модель не
    // регистрируется: окно пришлось бы объявить наугад, а это ровно то, от
    // чего мы уходим. Сказано вслух, а не обнаруживается в работе.
    if (!Number.isInteger(window) || window <= 0) {
        console.error(
            `llama-single: ${base} (${c.provider}) не назвал окно в meta.n_ctx — ` +
            "это не llama.cpp, и модели у pi от него не будет",
        );
        return null;
    }

    // Размышления объявляются по шаблону чата: `enable_thinking` в нём значит,
    // что модель размышляет и сервер умеет это выключать. Не ответил `/props`
    // — считаем, что не размышляет: ошибка в эту сторону стоит дешевле.
    let reasoning = false;
    try {
        const props = await getJson(`${base}/props`, headers);
        reasoning = typeof props?.chat_template === "string"
            && props.chat_template.includes("enable_thinking");
    } catch (e) {
        console.error(`llama-single: ${base} не ответил на /props (${e.message}), размышления не объявлены`);
    }

    return {
        name: "llama.cpp",
        baseUrl: `${base}/v1`,
        // Ключа у нашего сервера обычно нет. Поле непустое потому, что
        // провайдер без ключа считается ненастроенным; сервер без --api-key
        // принимает любой.
        apiKey: c.key || "llamacpp",
        api: "openai-completions",
        models: [{
            type: "chat",
            id: model.id,
            name: model.id,
            reasoning,
            input: ["text"],
            // Своя карта, счёт за токены никому не приходит.
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            contextWindow: window,
            maxTokens: Math.min(ANSWER_LIMIT, window),
        }],
    };
};

export default async function (pi) {
    // Ни одного адреса — провайдера нет, и это не ошибка: стенд без сервера
    // модели штатен, pi на нём запускается и сам говорит, что моделей нет.
    for (const c of candidates()) {
        const config = await ask(c);
        if (config) {
            pi.registerProvider("llamacpp", config);
            return;
        }
    }
}
