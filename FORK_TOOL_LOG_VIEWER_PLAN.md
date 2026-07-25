# План форка Codex с полным просмотром инструментов и reasoning summaries

## Статус выполнения

Обновлено: 2026-07-25.

### Инфраструктура форка

- [x] GitHub CLI авторизован как `mikhailsal`.
- [x] Форк `mikhailsal/codex` создан и проверен.
- [x] Remotes: `upstream` → `openai/codex`, `origin` → `mikhailsal/codex`.
- [x] `main` форка синхронизируется с `upstream/main` перед каждым этапом.

### Функциональные PR (порядок из §14)

| # | Тема | Ветка | PR | Статус |
| --- | --- | --- | --- | --- |
| 1 | Полный MCP transcript в `Ctrl+T` + banner усечения | `tool-transcript/01-mcp-full-transcript` | [#1](https://github.com/mikhailsal/codex/pull/1) | смержен 2026-07-25 |
| 2 | Reader и `ToolCallRecord` в `codex-session-inspector` | `tool-transcript/02-raw-model` | [#2](https://github.com/mikhailsal/codex/pull/2) | смержен 2026-07-25 |
| 3 | Truncation/completeness detector + TUI на общем API | `tool-transcript/03-completeness-detector` | [#3](https://github.com/mikhailsal/codex/pull/3) | открыт, готов к review 2026-07-25 |
| 4 | Function/custom full transcript в TUI | — | — | не начат |
| 5+ | Lazy pager, CLI, web, reasoning, transport A/B | — | — | не начат |

Текущая ветка: `tool-transcript/03-completeness-detector`.

### Что уже есть в `main` форка

- `Ctrl+T` показывает полный MCP result без лимита в 5 строк и banner при upstream truncation (ad-hoc substring check в TUI до PR #3).
- Crate `codex-session-inspector`: чтение plain/zstd rollout, pairing call↔output по `(turn_id, call_id)`, orphan outputs, unknown records как raw JSON.
- Characterization snapshots для MCP transcript.

### PR #3 (текущий этап) — цели и границы

**Входит:**

- модуль `completeness` в `codex-session-inspector`;
- поле `completeness` на `ToolCallRecord` / `OrphanToolOutput` (`Complete` / `Truncated { markers }` / `Unknown`);
- детектор известных Codex-маркеров усечения с метаданными (kind, count, byte offset, matched text);
- перевод TUI MCP transcript на `text_contains_truncation_marker` вместо локального heuristic;
- тесты на каждую семью маркеров и регрессию ложного срабатывания на `100 chars truncated…` без ведущего `…`.

**Не входит (следующие PR):**

- полный transcript для function/custom/shell cells;
- lazy pager для многомегабайтных outputs;
- CLI / web viewer;
- reasoning summaries и transport recorder.

**Семантика `Complete`:** в сохранённом тексте нет известного маркера, который Codex writers вставляют при discard. Это не доказательство, что исходный output был меньше любого лимита. Текст, который лишь *цитирует* маркер, может дать false positive; metadata markers делает такие случаи проверяемыми.

### Дальше после PR #3

1. Function/custom full transcript поверх `ToolCallRecord` + `completeness`.
2. CLI `codex debug session …`.
3. Loopback web viewer.
4. Reasoning summary command / provenance / transport A/B — отдельными PR.

## 1. Цель

Создать форк `mikhailsal/codex`, в котором:

1. основной чат остаётся компактным;
2. `Ctrl+T` показывает полные сохранённые аргументы и результаты вызовов инструментов;
3. интерфейс явно отличает полностью сохранённый результат от результата, который был обрезан
  до записи в session rollout;
4. те же данные можно удобно просматривать из отдельной консольной команды;
5. поверх общего безопасного backend можно запустить локальный веб-интерфейс.
6. reasoning summaries можно включать и переключать из TUI, а интерфейс объясняет, что именно
  запросили у модели и что фактически было получено.
7. для каждого inference request можно увидеть effective transport contract: обычный Responses
  или внутренний Responses Lite, точное сериализованное поле `reasoning`, безопасно сохранённые
   headers, request body и исходные streaming events;
8. Lite и обычный режим можно сравнить воспроизводимым A/B-тестом, не редактируя вручную
  `$CODEX_HOME/models_cache.json` и не выдавая внутренний режим за публичный API-параметр.

Под «полным» результатом понимается точный текстовый payload, который присутствует в
`$CODEX_HOME/sessions` или `$CODEX_HOME/archived_sessions`. Если в rollout уже записан маркер
вроде `Warning: truncated output` или `…N tokens truncated…`, отсутствующую часть восстановить
невозможно. Все интерфейсы должны явно показывать такую потерю данных.

## 2. Исходное состояние

- Локальный checkout: `openai/codex`, ветка `main`.
- Текущий remote `origin`: `https://github.com/openai/codex`.
- Предполагаемый GitHub-профиль: `mikhailsal`.
- Текущая авторизация `gh` недействительна; перед созданием форка потребуется `gh auth login`.
- Rollout-файлы уже содержат `function_call`, `function_call_output`, `custom_tool_call`,
`custom_tool_call_output`, MCP-события и события выполнения команд.
- `Ctrl+T` использует `HistoryCell::transcript_lines()`, но большинство cells наследуют
компактный `display_lines()`.
- MCP renderer обрезает текст до `TOOL_CALL_MAX_LINES`, равного пяти строкам.
- Shell `ExecCell` имеет отдельный расширенный transcript renderer, но и он может показать только
ту часть, которая была сохранена upstream.

## 3. Принципы реализации

- Не менять compact view основного чата.
- Не переписывать историю и не менять model-visible context ради UI.
- Не увеличивать `tool_output_token_limit` как способ исправления интерфейса: это другой слой.
- Не добавлять новую функциональность в `codex-core`, если её можно разместить в отдельном crate
или в существующем `codex-rollout`.
- Минимизировать постоянный diff относительно `openai/codex`, чтобы обновления upstream можно
было регулярно сливать без ручного переноса функциональности.
- Использовать один parser и одну нормализованную модель данных для TUI, консольного viewer и web.
- Читать большие результаты лениво или постранично; отсутствие UI-обрезки не должно означать
загрузку многогигабайтного файла в один `String`.
- Поддерживать обычные `.jsonl` и сжатые `.jsonl.zst`.
- Сохранять совместимость с Linux, macOS и Windows.
- Не скрывать ошибки разбора: malformed или неизвестные records показывать как raw JSON.
- Считать session-файлы чувствительными данными: они могут содержать токены, приватный код,
команды, пути и ответы внешних сервисов.
- Всегда различать provider-generated reasoning summary, открытый raw reasoning, непрозрачный
encrypted reasoning и реконструкцию по видимому trace. Не называть реконструкцию внутренним
chain of thought.
- Считать `use_responses_lite` внутренним полем каталога моделей, а не публичным параметром
Responses API или стабильной пользовательской настройкой.
- Не считать слово `lite` признаком короткого или усечённого ответа: это отдельный transport
contract, влияние которого на summaries должно подтверждаться измерением.
- Не изменять автоматически скачанный model cache для экспериментов. Использовать явный
session-scoped override или отдельный каталог, источник и hash которого записываются в
диагностику.
- Отделить захват transport-данных от TUI: один observer/recording layer должен обслуживать
session-файл, консольный viewer, web viewer и A/B harness.

## 4. GitHub и ветвление

### 4.1. Подготовка форка

```bash
gh auth login -h github.com
gh auth status
gh repo fork openai/codex --clone=false
git remote rename origin upstream
git remote add origin git@github.com:mikhailsal/codex.git
git fetch upstream
git fetch origin
```

Если SSH не настроен, использовать HTTPS URL форка. Перед изменением remote проверить результат
`gh repo view mikhailsal/codex`; не предполагать, что форк был создан успешно.

### 4.2. Ветки

Не делать весь проект одним большим PR. Фактические ветки форка:

1. `tool-transcript/01-mcp-full-transcript` — PR #1 (смержен)
2. `tool-transcript/02-raw-model` — PR #2 (смержен)
3. `tool-transcript/03-completeness-detector` — PR #3 (текущий)
4. `tool-transcript/04-tui-function-custom` — следующий TUI transcript
5. `tool-transcript/05-session-cli`
6. `tool-transcript/06-local-web`
7. `tool-transcript/07-console-ux` / reasoning / transport — по мере готовности

Каждая ветка создаётся от актуального `upstream/main`. Изменения предыдущего этапа либо сначала
сливаются в `main` форка, либо последующие PR явно оформляются как stacked PR.

Git hooks не отключать и не обходить. Коммиты делать небольшими и тематическими.

### 4.3. Стратегия минимального расхождения с upstream

Форк должен оставаться расширением Codex, а не отдельной переписанной версией. Для этого:

- сохранять `upstream` как отдельный remote, указывающий на `openai/codex`;
- регулярно обновлять локальную `main` только из `upstream/main`;
- держать fork-specific изменения в небольших тематических commits и PR;
- не смешивать функциональные изменения с переименованиями, форматированием или массовым
рефакторингом существующего upstream-кода;
- по возможности добавлять новые модули и crates вместо изменения центральных больших файлов;
- использовать существующие traits и extension points, а при их нехватке добавлять минимальный
общий extension point отдельным commit;
- избегать копирования крупных upstream-модулей в fork-specific версии;
- не менять формат rollout, app-server API и config schema без необходимости;
- делать изменения пригодными для отдельного PR в `openai/codex`, даже если отправка upstream
произойдёт позже;
- отмечать fork-only код коротким единообразным module-level комментарием, но не засорять каждую
строку маркерами форка;
- хранить web UI и session inspector максимально изолированно от TUI orchestration;
- не изменять generated files вручную; регенерировать их штатными командами Codex;
- сохранять тесты рядом с новым кодом, чтобы merge-конфликты в центральных test modules были
редкими.

Рекомендуемый цикл синхронизации:

```bash
git fetch upstream
git switch main
git merge --ff-only upstream/main
git push origin main
```

Для незавершённых feature branches:

```bash
git fetch upstream
git switch tool-transcript/<branch>
git rebase upstream/main
```

Опубликованные совместные ветки не переписывать без согласования; для них использовать merge из
`upstream/main`. Выбор rebase или merge не должен приводить к потере истории или обходу Git hooks.

Перед каждым заметным этапом:

1. синхронизировать `main` форка с `upstream/main`;
2. выполнить пробный merge/rebase feature branch;
3. сначала разрешить конфликты без изменения поведения;
4. запустить затронутые тесты;
5. отдельным commit адаптировать fork-функциональность к новым upstream API.

Не реже одного раза в неделю во время активной разработки выполнять dry-run синхронизации. Если
одно и то же место конфликтует повторно, это сигнал вынести fork-код из центрального модуля или
предложить upstream небольшой стабильный extension point.

## 5. Этап 0: зафиксировать поведение и контракт

### Задачи

- Составить матрицу всех типов вызовов:
  - shell/unified exec;
  - `function_call`;
  - `custom_tool_call`;
  - MCP;
  - web search;
  - tool search;
  - image/audio outputs;
  - agent collaboration tools.
- Для каждого типа записать:
  - где лежат аргументы;
  - где лежит результат;
  - какой live event получает TUI;
  - что сохраняется в rollout;
  - где происходит storage truncation;
  - что сейчас отображают compact view и `Ctrl+T`.
- Подготовить безопасные synthetic fixtures без пользовательских session-файлов.
- Зафиксировать текущий UI snapshot-тестами, включая MCP-ответ длиннее пяти строк.
- Описать критерии «полный», «обрезан upstream», «binary/не отображаемый как текст».

### Результат

Небольшой тестовый PR без изменения поведения. Он защищает последующие изменения от регрессий и
не использует реальные пользовательские логи как fixtures.

## 6. Этап 1: общая модель сырых tool records

### Размещение

Предпочтительный вариант — новый небольшой crate, например
`codex-rs/session-inspector`, который зависит от `codex-protocol` и `codex-rollout`, но не от
`codex-core`.

Если после исследования окажется, что нужный parser естественно расширяет `codex-rollout`, оставить
низкоуровневое чтение там, а нормализацию records — в новом crate.

### Модель

Ввести внутреннее представление наподобие:

```text
ToolCallRecord
├── identity: thread/turn/call/item IDs
├── tool: kind, namespace/server, name
├── timing: started/completed/duration
├── status
├── arguments: raw payload + optional parsed JSON
├── result: text/structured/image/audio/resource/error
├── persistence source: file + ordinal/offset
└── completeness: complete/truncated/unknown + marker metadata
```

Это не новая публичная app-server API на первом этапе. Тип должен оставаться приватным для crate,
пока контракт не стабилизируется.

### Требования

- Сопоставлять call и output по `call_id`.
- Не терять raw JSON при успешном pretty-print.
- Сохранять порядок records.
- Выявлять известные маркеры усечения структурно и текстово.
- Не считать payload полным только потому, что маркер отсутствует: использовать состояние
`unknown`, когда источник не даёт гарантии.
- Уметь читать только метаданные, диапазон records и выбранный result без полного чтения файла.
- Возвращать понятные ошибки для повреждённых JSONL и zstd.

### Тесты

- Полный function/custom/MCP result.
- Structured content.
- Длинный многострочный результат.
- Известные truncation markers.
- Несопоставленный call/output.
- Повторный `call_id` в разных turns.
- Неизвестный record type.
- Повреждённая строка JSONL.
- Сжатый rollout.
- Пути и текст с Unicode.

## 7. Этап 2: полный transcript по `Ctrl+T`

### Минимальное полезное изменение

- Оставить `display_lines()` компактным.
- Реализовать отдельный `transcript_lines()` для MCP и остальных tool-call cells.
- В transcript показывать:
  - полное имя инструмента;
  - `call_id`;
  - точные аргументы;
  - status и duration;
  - полный сохранённый текст результата;
  - structured content как pretty JSON;
  - exit code для команд;
  - заметный banner при upstream truncation.
- Не применять `TOOL_CALL_MAX_LINES` внутри transcript renderer.
- Для изображения, аудио и resource content показывать тип, MIME, размер и безопасное действие
просмотра/сохранения, а не печатать base64 в терминал по умолчанию.

### Большие результаты

Pager overlay нельзя заставлять заранее строить `Vec<Line>` для всего многомегабайтного результата.
Перед включением truly full output:

- добавить ленивый источник строк или чанков;
- держать небольшой viewport cache;
- загружать данные по мере прокрутки;
- показывать прогресс чтения;
- позволить отменить загрузку;
- не блокировать event loop TUI;
- сохранить поиск по всему result как отдельную отменяемую background operation.

### Управление

Предлагаемые действия внутри `Ctrl+T`:

- `Enter` — свернуть/развернуть выбранный tool call;
- `a` — показать аргументы;
- `o` — показать output;
- `r` — raw/pretty JSON;
- `/` — поиск;
- `n`/`N` — следующий/предыдущий результат;
- `y` — копировать выбранный блок;
- `e` — экспортировать выбранный payload в файл;
- `g`/`G`, `PageUp`/`PageDown` — навигация;
- `Esc` или `Ctrl+T` — закрыть overlay.

Все клавиши должны проходить через существующий keymap, а не быть захардкожены без возможности
переназначения.

### Настройка

Если понадобится совместимость, добавить явную настройку:

```toml
[tui]
transcript_tool_detail = "full" # compact | full
```

Предпочтительный default для `Ctrl+T` — `full`, поскольку основной чат уже обеспечивает compact
view. Raw binary/base64 остаётся скрытым до отдельного действия.

### Reasoning summaries и наблюдаемость

Существующий transport Codex уже поддерживает `reasoning.summary` и streaming
`ReasoningSummaryTextDelta`, поэтому не нужно создавать отдельный несовместимый протокол. Форк
должен улучшить управление, диагностику и представление этих данных.

Добавить slash-команду:

```text
/reasoning-summary auto|concise|detailed|off
```

Команда должна:

- менять настройку текущей сессии без ручного редактирования TOML;
- явно показывать effective значение;
- предлагать сохранить выбор глобально только отдельным подтверждённым действием;
- не обещать summary, если каталог модели не объявляет поддержку параметра;
- не влиять на `model_reasoning_effort`, поскольку effort и summary — разные настройки.

В `/status` показывать отдельный блок:

```text
Reasoning summary requested: detailed
Model accepts parameter: yes
Summaries received this turn: 4
Raw reasoning received: no
Encrypted reasoning present: yes
```

Значения должны строиться по реальным request/event данным. Не выводить `yes` только на основании
настройки пользователя.

В `Ctrl+T`:

- показывать все reasoning summary parts полностью;
- сохранять границы summary sections;
- связывать summary с reasoning item, turn и последующими tool calls;
- позволять свернуть весь reasoning turn или отдельную section;
- показывать source/provenance рядом с блоком;
- не смешивать summary и raw reasoning в один неразличимый поток;
- после resume отображать те же блоки, которые были видны live.

Ввести явные категории provenance:

- `provider_summary` — summary, возвращённый моделью;
- `raw_reasoning` — открытый reasoning content, реально полученный от provider;
- `encrypted_reasoning` — наличие opaque encrypted state без попытки расшифровки;
- `trace_summary` — отдельная реконструкция по наблюдаемым событиям.

Для модели без native summaries можно добавить opt-in второй проход, который суммирует только
видимый trace: сообщения, планы, вызовы инструментов и результаты. Такой режим:

- выключен по умолчанию;
- явно называется `trace summary`, а не reasoning;
- показывает модель и момент генерации;
- не утверждает, что раскрывает внутренний chain of thought;
- не получает encrypted reasoning;
- имеет отдельный бюджет и отмену;
- не записывается как provider summary;
- может быть полностью отключён политикой или конфигурацией.

Если summary был запрошен, но не пришёл, UI должен показать диагностическое состояние, например:

```text
Detailed reasoning summary was requested, but this turn returned no summary.
```

Диагностика должна различать как минимум:

- модель не принимает параметр;
- effective setting равен `none`;
- summary запрошен, но response не содержит summary;
- summary получен пустым;
- summary был получен, но скрыт пользовательской UI-настройкой;
- rollout содержит encrypted reasoning без открытого summary.

### Responses Lite и точная диагностика inference request

`use_responses_lite` — реально существующее поле `ModelInfo`, но не публичное поле тела Responses
API. В текущем Codex оно выбирает внутренний transport contract и приводит как минимум к следующим
изменениям:


| Обычный Responses                                 | Responses Lite                                                          |
| ------------------------------------------------- | ----------------------------------------------------------------------- |
| `instructions` передаются верхнеуровневым полем   | инструкции добавляются как `developer` item в `input`                   |
| инструменты передаются верхнеуровневым `tools`    | инструменты передаются через `additional_tools` item                    |
| hosted Responses tools могут выполняться provider | web/image tools маршрутизируются через Codex-owned standalone executors |
| `parallel_tool_calls` зависит от модели и prompt  | Codex принудительно передаёт `false`                                    |
| `reasoning.context` обычно не задаётся Codex      | Codex задаёт `reasoning.context: "all_turns"`                           |
| сохраняется поддерживаемый `image.detail`         | `image.detail` удаляется из копии request                               |
| внутреннего Lite marker нет                       | передаётся `X-OpenAI-Internal-Codex-Responses-Lite: true`               |


Название Lite не означает, что response или reasoning summary должны быть короче. Если Codex
сериализует `reasoning.summary: "detailed"`, а provider возвращает короткие heading-only summaries,
UI должен показывать это как наблюдаемое расхождение `requested detailed / received N bytes`, не
объявляя заранее причиной Lite, модель или TUI.

Добавить единый inference transport record:

```text
InferenceTransportRecord
├── identity: thread/turn/request/response IDs
├── model: requested slug + resolved catalog entry/hash/source
├── route: provider, endpoint class, HTTP or WebSocket
├── contract: responses | responses_lite
├── request headers: exact names/values after mandatory secret redaction
├── request body: exact serialized JSON before compression/transmission
├── response headers: exact safe subset + redacted raw form
├── stream: ordered raw SSE/WebSocket events and decoded event type
├── reasoning request: effort, summary, context, mode, include, stream options
├── reasoning response: item/part boundaries, text byte count, encrypted-content presence
├── tools contract: top-level tools or additional_tools, parallel mode
├── timing/retry/connection attempt
└── completeness/redaction/storage metadata
```

Для фразы «точный raw request» определить границу явно: сохранять байты сериализованного body,
отправленные transport layer, и headers непосредственно перед отправкой. Authorization, cookies,
API keys, session/access tokens и иные credentials должны заменяться типизированными redaction
records до записи на диск. Нельзя сначала записать секрет, а затем пытаться очистить файл.

Raw streaming log должен сохранять:

- порядок событий без слияния соседних delta;
- event name/type, исходный payload и локальное время получения;
- границы response items и reasoning summary parts;
- terminal event, disconnect, retry и переход HTTP/WebSocket;
- признак, было ли событие сохранено полностью или потеряно до recorder;
- связь decoded UI event с исходным raw event.

В обычном rollout по умолчанию можно хранить безопасную структурированную запись. Полный
transport capture сделать явным диагностическим режимом с понятным предупреждением о размере и
чувствительности данных, ограничением дискового бюджета и политикой retention. Режим должен быть
доступен для одной сессии без постоянного глобального включения.

Добавить экспериментальное управление текущей сессией:

```text
/responses-contract default|standard|lite
```

- `default` использует значение из resolved model catalog и остаётся режимом по умолчанию;
- `standard` и `lite` — диагностические overrides, а не обещанные публичные возможности;
- override показывает предупреждение, если выбранный provider/model не объявляет совместимость;
- переключение между Lite и standard закрывает несовместимое cached WebSocket connection;
- effective contract отображается в `/status` и записывается в session;
- изменение не должно переписывать `$CODEX_HOME/models_cache.json`;
- для автоматизации предусмотреть эквивалентный session-scoped CLI flag;
- если backend отверг override, сохранить точный status/error и предложить вернуться к `default`,
но не делать молчаливый fallback, который испортит A/B-сравнение.

Предпочтительно реализовать override поверх копии resolved `ModelInfo` в turn/session context.
Если для первого прототипа потребуется `model_catalog_json`, создавать отдельный явно указанный
каталог и сохранять его hash/source; не советовать пользователю помечать автоматически обновляемый
cache read-only.

Добавить воспроизводимый A/B harness:

```bash
codex debug responses compare \
  --model gpt-5.6-sol \
  --reasoning-effort medium \
  --reasoning-summary detailed \
  --prompt-file prompt.txt
```

Он должен запускать две свежие изолированные сессии с одинаковыми:

- model slug и catalog revision;
- prompt, base instructions и набором tools;
- reasoning effort/summary и остальными публичными параметрами;
- auth/provider/endpoint и service tier;
- историей либо явно пустым context;
- лимитами времени и числом повторов.

Единственной плановой переменной должен быть `responses` против `responses_lite`. Поскольку Lite
принудительно меняет `reasoning.context`, parallel tools и представление instructions/tools, отчёт
должен показывать эти неизбежные contract differences, а не утверждать, что изменён только один
JSON boolean. Для статистически полезного результата поддержать несколько повторов и сравнивать:

- число reasoning items и summary parts;
- длину каждого summary в bytes/chars/tokens;
- heading-only против heading+body;
- latency до первого summary и полного ответа;
- tool calls, retries и terminal status;
- usage и encrypted reasoning presence;
- нормализованный diff request body, headers и raw event sequence.

Каждый вывод о причине коротких summaries маркировать уровнем доказательности:

- `observed` — присутствует в записанных request/response bytes;
- `source-derived` — следует из конкретной версии исходников Codex;
- `documented` — подтверждён публичной документацией API;
- `hypothesis` — требует контролируемого A/B или ответа provider.

В web и консольном viewer добавить экран сравнения двух inference records: нормализованный diff,
переключатель raw/decoded, фильтр secrets/redactions и отдельную вкладку Reasoning. Это позволит
проверять не только Lite, но и последующие изменения model catalog или transport без новой
специализированной диагностики.

### Проверка

- `insta` snapshots для каждого tool type.
- Snapshot с truncation banner.
- Snapshot raw и pretty JSON.
- Интеграционный тест live call и replay после resume.
- Проверка одинакового результата для активной и возобновлённой сессии.
- Тест `/reasoning-summary` для всех четырёх режимов.
- Тест модели без поддержки summary parameter.
- Тест «summary запрошен, но не возвращён».
- Тест provider summary, raw reasoning и encrypted-only reasoning.
- Тест provenance и отдельной маркировки opt-in trace summary.
- Тест live/resume parity для reasoning summary parts.
- Тест точной сериализации standard и Lite request contracts.
- Тест `reasoning.summary: detailed` в обоих contracts.
- Тест redaction headers/body до записи transport log на диск.
- Тест raw SSE/WebSocket event ordering и связи с decoded UI events.
- Тест session-scoped override без изменения model cache.
- Тест закрытия/reconnect WebSocket при смене contract.
- Тест явной ошибки вместо молчаливого fallback при неподдерживаемом Lite.
- Golden-отчёт A/B harness с единственным выбранным contract и перечислением неизбежных
производных различий.
- Проверка resize/reflow и узкого терминала.
- Интерактивная проверка по инструкции `test-tui`.
- `just test -p codex-tui`, затем `just fix -p codex-tui` и `just fmt`.

## 8. Этап 3: удобный консольный session viewer

### Командный интерфейс

Сначала добавить экспериментальный интерфейс, не обещая стабильный wire format:

```bash
codex debug session list
codex debug session show --last
codex debug session show <THREAD_ID>
codex debug session tools <THREAD_ID>
codex debug session tool <THREAD_ID> <CALL_ID>
codex debug session export <THREAD_ID> --format jsonl
```

После стабилизации решить, стоит ли повышать его до `codex session ...`.

### Возможности

- Таблица сессий: дата, cwd, branch, model, duration, число tool calls, размер.
- Фильтры по tool name, status, времени, turn и наличию truncation.
- Фильтры по наличию provider summary, raw/encrypted reasoning и trace summary.
- `--raw`, `--pretty`, `--json`, `--jsonl`.
- `--arguments-only`, `--output-only`.
- `--full` и `--head`/`--tail` для контролируемого вывода в pipe.
- Цвет только при TTY; чистый stdout при перенаправлении.
- Стабильные exit codes для missing session/call, parse error и truncated result.
- Поддержка `--last`, thread ID и session name.
- Вывод предупреждения о чувствительных данных перед массовым export.

### Интерактивный режим

Поверх той же библиотеки можно добавить `codex debug session tui`, но только после появления
работающего неинтерактивного CLI. Это позволит тестировать parser без привязки к UI и использовать
его в скриптах.

## 9. Этап 4: локальный веб-интерфейс

### Архитектура

```text
rollout JSONL/ZST
        │
        ▼
session-inspector Rust library
        │
        ├── Codex TUI transcript
        ├── console session viewer
        └── loopback HTTP server ──► browser UI
```

Не читать rollout повторно в JavaScript. Backend должен отдавать нормализованные records и
постраничные chunks результата.

### Запуск

```bash
codex debug session web --last
codex debug session web <THREAD_ID>
```

По умолчанию:

- bind только на `127.0.0.1`;
- случайный свободный порт;
- короткоживущий случайный access token;
- URL с token открывается локально;
- отсутствие CORS для внешних origins;
- read-only режим;
- запрет произвольных filesystem paths.

Любой bind на non-loopback должен требовать явного флага и предупреждения.

### Первая версия UI

- Timeline user/assistant/tool событий.
- Reasoning summary sections с provenance и диагностикой requested/received.
- Сворачиваемые tool calls.
- Вкладки Arguments, Result, Raw JSON, Metadata.
- Pretty JSON с возможностью перейти к raw bytes/text.
- Поиск по session и внутри одного output.
- Фильтры по типу инструмента и status.
- Признак complete/truncated/unknown.
- Сравнение compact TUI preview с сохранённым полным payload.
- Копирование и скачивание выбранного блока.
- Deep link на `call_id` без включения содержимого в URL.
- Virtualized rendering для больших outputs.
- Светлая и тёмная тема, клавиатурная навигация.

### Безопасность

- CSP без внешних scripts/styles.
- Никаких CDN и telemetry.
- HTML-экранирование всех данных session.
- Ссылки и ANSI не должны превращаться в исполняемый HTML.
- Base64 и binary content не рендерить автоматически.
- Явная кнопка Reveal для потенциальных секретов.
- Опциональная локальная redaction только на уровне представления; оригинальный rollout не менять.
- Export всегда предупреждает, что redaction view не гарантирует очистку исходного файла.

### API

На первом этапе использовать приватные loopback endpoints. Если позже понадобится интеграция с
Codex app-server, добавлять только v2 experimental API, с pagination и schema generation. До этого
не расширять публичный app-server контракт.

## 10. Этап 5: улучшение консольного UX

- Синхронизировать навигацию standalone viewer и `Ctrl+T`.
- Добавить быстрый переход к следующему failed/truncated tool call.
- Показывать размер полного payload до раскрытия.
- Добавить folding JSON-дерева.
- Подсвечивать stdout/stderr отдельно.
- Поддержать сохранение пользовательского фильтра только в UI config, не в session.
- Добавить безопасное открытие exported payload во внешнем editor.
- Показывать источник: live event, rollout response item или reconstructed record.
- Для неизвестных records давать raw fallback вместо молчаливого пропуска.
- Добавить команду диагностики, сравнивающую число calls и outputs и находящую orphan records.

## 11. Наблюдаемость и производительность

Измерять без записи содержимого:

- время открытия session;
- время до первого отображённого record;
- число records;
- прочитанные и декомпрессированные байты;
- размер viewport cache;
- parse errors;
- orphan calls/outputs;
- число detected truncations.

Никогда не отправлять в telemetry аргументы, output, пути, tool names внешних MCP-серверов или raw
JSON.

Целевые показатели первой версии:

- список сессий появляется менее чем за секунду на локальном каталоге порядка сотен файлов;
- первый экран выбранной сессии не требует чтения всех больших outputs;
- прокрутка не блокирует TUI;
- memory usage зависит от viewport/chunk cache, а не от полного размера rollout.

## 12. Совместимость и миграция

- Не изменять существующий формат rollout на первых этапах.
- Старые session-файлы должны оставаться читаемыми.
- Не переписывать session при просмотре.
- Не заменять provider summary позднее сгенерированным trace summary.
- Поддерживать archived и compressed sessions.
- Не считать порядок JSON object keys частью контракта.
- Не обещать восстановление данных, которые уже заменены truncation marker.
- Если в будущем потребуется сохранять больше данных, сделать отдельное обсуждение retention,
дискового бюджета, секретов и backward compatibility.

## 13. План тестирования

### Unit

- Парсинг и pairing records.
- Truncation detector.
- Raw/pretty renderers.
- Pagination/chunk boundaries.
- ANSI и Unicode.
- Path handling на трёх ОС.

### Integration

- Live tool call появляется в `Ctrl+T` полностью.
- После restart/resume отображение совпадает.
- Reasoning summary setting доходит до request, а фактически полученные summaries попадают в
live UI и rollout replay.
- Transport recorder показывает, был ли фактически использован Responses Lite, какое
`reasoning.summary/context` было сериализовано и какие raw summary events вернул provider.
- Standard/Lite A/B harness не изменяет model cache и сохраняет воспроизводимый diff.
- Отсутствующий summary диагностируется без попытки показать encrypted reasoning.
- CLI и web backend возвращают одинаковую нормализованную запись.
- Чтение `.jsonl.zst`.
- Повреждённая строка не скрывает остальные доступные records.
- Большой synthetic output читается лениво.

### UI snapshots

- Compact chat не изменился.
- Full transcript для shell, function, custom и MCP.
- Structured result.
- Truncated result.
- Error result.
- Binary/image placeholder.
- Узкий terminal и resize.

### Security

- HTML/script injection в arguments/result.
- ANSI/OSC escape injection.
- Path traversal.
- Попытка web bind наружу без явного разрешения.
- Утечка access token через logs или referrer.

## 14. Размер и порядок PR

Каждый немеханический PR держать значительно меньше 800 изменённых строк; сложную логику — ближе
к 500 строкам. Разбиение и статус:

1. [x] fixtures и characterization tests — внутри PR #1;
2. [x] reader и `ToolCallRecord` — PR #2;
3. [~] truncation/completeness detector — PR #3 (текущий);
4. [ ] MCP full transcript — фактически сделан в PR #1; banner переведён на общий detector в PR #3;
5. [ ] function/custom full transcript;
6. [ ] lazy pager;
7. [ ] CLI list/show;
8. [ ] CLI filters/export;
9. [ ] loopback backend;
10. [ ] минимальный web UI;
11. [ ] virtualized output и поиск;
12. [ ] UX polishing;
13. [ ] reasoning summary command и effective-status diagnostics;
14. [ ] reasoning provenance в TUI/CLI/web;
15. [ ] общий redacted inference transport recorder;
16. [ ] session-scoped Responses contract override и `/status`;
17. [ ] standard/Lite A/B harness и compare view;
18. [ ] отдельный opt-in trace-summary prototype.

Не смешивать schema/API change, parser, TUI redesign и web UI в одном PR.

## 15. Критерии готовности

Первая полезная версия считается готовой, когда:

- `Ctrl+T` показывает точные сохранённые arguments и text results для всех поддерживаемых tool
calls;
- нет пятистрочного лимита внутри transcript;
- upstream truncation заметен и не маскируется;
- live и resumed session выглядят одинаково;
- большие outputs не блокируют TUI;
- CLI может найти session и экспортировать выбранный call;
- reasoning summary можно переключить из TUI, а `/status` показывает requested, supported и
received состояния;
- `/status` и `Ctrl+T` показывают effective Responses contract и фактически сериализованные
reasoning parameters;
- opt-in transport capture сохраняет точный request body и ordered raw stream без credentials;
- standard/Lite A/B можно запустить без ручного изменения model cache;
- provider summary, raw/encrypted reasoning и trace summary визуально не смешиваются;
- реальные пользовательские session-файлы не попали в git или test fixtures.

Веб-версия считается готовой, когда:

- работает только локально и read-only по умолчанию;
- использует тот же parser;
- показывает timeline, arguments, result, raw JSON и completeness;
- безопасно обрабатывает HTML/ANSI/binary;
- не требует внешних CDN или облачного сервиса.

## 16. Первый рабочий спринт

1. [x] Восстановить `gh`-авторизацию и создать `mikhailsal/codex`.
2. [x] Добавить remote форка, не потеряв ссылку на `openai/codex` как `upstream`.
3. [x] Создать characterization fixtures и snapshots.
4. [x] Реализовать отдельный полный MCP transcript без пятистрочного лимита.
5. [x] Добавить banner для уже обрезанного результата.
6. [x] Проверить live и resume поведение.
7. [ ] Собрать локальный бинарник и протестировать на копии одной собственной session — бинарник
  собран; ручная проверка session остаётся.
8. [x] Открыть и смержить PR в форке с описанием ограничений и следующего этапа:
  [mikhailsal/codex#1](https://github.com/mikhailsal/codex/pull/1), смержен 2026-07-25.

После этого отдельно принимать решение о shared session-inspector crate. Не начинать web UI до
того, как TUI и CLI читают одну и ту же нормализованную запись без потери данных.

Следующим независимым спринтом реализовать `/reasoning-summary`, effective-status diagnostics и
полное отображение уже сохранённых provider summaries. Opt-in trace summary начинать только после
этого и не смешивать с базовым PR.