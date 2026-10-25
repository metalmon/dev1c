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

// Адрес и ключ — из окружения, которое точка входа кладёт в /etc/environment:
// LLAMA_API_URL — наш сервер (контейнер рядом или служба на хосте),
// CUSTOM_API_URL — «удалённый», то есть не нами поставленный. Для расширения
// разницы нет: лишь бы это был llama.cpp, у него `/props` и `meta.n_ctx` есть
// всегда. Имена переменных свои, а не LLAMA_BASE_URL/LLAMA_API_KEY: те читает
// родной провайдер, и заводить их значило бы обещать router-режим.
const URL_VARS = ["LLAMA_API_URL", "CUSTOM_API_URL"];
const KEY_VARS = ["LLAMACPP_API_KEY", "CUSTOM_API_KEY"];

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

const firstSet = (names) => {
    for (const n of names) {
        const v = process.env[n];
        if (v && v.trim()) return v.trim();
    }
    return "";
};

const getJson = async (url, headers) => {
    const res = await fetch(url, { headers, signal: AbortSignal.timeout(TIMEOUT_MS) });
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    return res.json();
};

export default async function (pi) {
    const raw = firstSet(URL_VARS);
    // Адреса нет — провайдера нет, и это не ошибка: стенд без сервера модели
    // штатен, pi на нём запускается и сам говорит, что моделей нет.
    if (!raw) return;

    // Завершающий `/v1` снимается: родной клиент pi поступает так же, а в
    // dev1c.toml адрес встречается в обеих формах.
    const base = raw.replace(/\/+$/, "").replace(/\/v1$/, "");
    const key = firstSet(KEY_VARS);
    const headers = key ? { Authorization: `Bearer ${key}` } : {};

    let models;
    try {
        models = await getJson(`${base}/models`, headers);
    } catch (e) {
        console.error(`llama-single: сервер модели ${base} не ответил (${e.message})`);
        return;
    }

    const model = models?.data?.[0];
    const window = model?.meta?.n_ctx;
    // Нет `meta.n_ctx` — это не llama.cpp, а чужой шлюз. Модель не
    // регистрируется вовсе: окно пришлось бы объявить наугад, а это ровно то,
    // от чего мы уходим. Сказано вслух, а не обнаруживается в работе.
    if (!model?.id || !Number.isInteger(window) || window <= 0) {
        console.error(
            `llama-single: ${base}/models не назвал окно в meta.n_ctx — ` +
            "это не llama.cpp, и модели у pi не будет",
        );
        return;
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
        console.error(`llama-single: ${base}/props не ответил (${e.message}), размышления не объявлены`);
    }

    pi.registerProvider("llamacpp", {
        name: "llama.cpp",
        baseUrl: `${base}/v1`,
        // Ключа у нашего сервера обычно нет. Поле непустое потому, что
        // провайдер без ключа считается ненастроенным; сервер без --api-key
        // принимает любой.
        apiKey: key || "llamacpp",
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
    });
}
