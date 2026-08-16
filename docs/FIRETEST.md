# FireTest

Дата актуализации: 2026-07-31.

FireTest — встроенный debug/test scenario runner для поиска дорогих UI-ошибок в горячем chart path.

## Запуск Windows

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc"
Remove-Item -ErrorAction SilentlyContinue firetest.log, logs\render_diag.log
.\target\x86_64-pc-windows-msvc\debug\moonterminal.exe --debug-script chart-smoke
```

`chart-smoke` — один связный поведенческий прогон. Он ждёт старт приложения, открывает BTC-график, находит реальные bounds графика, даёт live-графику короткую settle-фазу, прогревает high-present baseline без курсора, а потом 5 секунд двигает системную мышь по графику частым native mousemove storm. После этого включает static text stress на графике и повторяет mouse storm. Только после горячего chart path FireTest проверяет runtime-контракт ошибок доставки команд в core, открывает tool-окна Settings/Strategies/Assets и проверяет их dedup, проверяет Root-owned overlay слой на реальном окне, затем переключает язык интерфейса живым apply-путём (`rust_i18n::set_locale` + `refresh_windows`) и проверяет его долёт без пересоздания tool-окон, проверяет прохождение масштаба `50% → 20% → Auto` до активного chart state, а затем может выполнить opt-in тест постановки и отмены реального BTC-ордера. На Windows storm делается через реальный `SetCursorPos` в client-area окна, на macOS — через CoreGraphics mouse move events. В обоих случаях это настоящий оконный input path, а не прямой вызов chart API.

`order-cancel-lag` — отдельный узкий сценарий для расследования только пути ордера. Он делает `start → open_chart → wait_chart_probe → settle → order_cancel_lag → cooldown` и автоматически включает реальный place/cancel test без mouse storm, static text и tool-window стадий. Использовать только осознанно: сценарий отправляет торговую команду в выбранное ядро.

На macOS тестовой машине может понадобиться выдать терминалу/приложению право Accessibility или Input Monitoring: это политика macOS для программной отправки событий мыши.

## Настройки

- `MOON_FIRETEST_MARKET` — рынок, по умолчанию `BTCUSDT`.
- `MOON_FIRETEST_MOUSE_HZ` — целевая частота mousemove storm, по умолчанию `5000`.
- `MOON_FIRETEST_STORM_MS` — длительность storm, по умолчанию `5000`.
- `MOON_FIRETEST_TEXT_LABELS` — число retained text labels в static text stress, по умолчанию `10000`.
- `MOON_FIRETEST_FLASH_MS` — длительность окна стадии `arrival_flash`, по умолчанию `5000` (столько же, сколько меряет `idle_floor` — окна сравниваются между собой). Первые 1200 мс окна не сэмплируются, так что по умолчанию в зачёт идут ~4 секунды; хотите больше точек — поднимайте эту переменную, а не ждите большего от дефолта.
- `MOON_ARRIVAL_FLASH=0` — глобально выключает рамку прибытия нового графика (`0`/`false`/`no`/`off`). Это НЕ FireTest-переменная: она читается самим приложением один раз при старте и действует и в обычном запуске. Контрольная сторона A/B-замера рамки.
- `MOON_FIRETEST_ORDER_CANCEL=1` — включает реальный тест place/cancel ордера на открытом BTC-графике. По умолчанию выключен, чтобы обычный FireTest не отправлял торговые команды.
- `MOON_FIRETEST_ORDER_SIZE` — явный размер тестового ордера в базовой монете. Если не задан, берётся `MOON_FIRETEST_ORDER_QUOTE_SIZE / order_price`, а если не задан и quote-size — групповой USD-эквивалент переводится в базовую монету по текущему base/USD rate. При отсутствии корректного курса сценарий завершится ошибкой, а не отправит размер как есть.
- `MOON_FIRETEST_ORDER_QUOTE_SIZE` — размер тестового ордера в котируемой валюте (для BTCUSDT это USDT). Например `500` при `MOON_FIRETEST_ORDER_PRICE_MULT=0.95` даст количество `500 / (latest_price * 0.95)`.
- `MOON_FIRETEST_ORDER_PRICE_MULT` — множитель к последней цене для тестового long-limit ордера, по умолчанию `0.98`. Ордер ставится ниже рынка, чтобы тест проверял отображение/отмену, а не случайное исполнение.
- `MOON_FIRETEST_ORDER_CANCEL_MAX_DISPLAY_MS` — допустимая задержка от применения cancelled order в store до первого chart present/draw с этой order-line revision, по умолчанию `750`.
Static text stress входит в стандартный `chart-smoke`: FireTest сам включает
`10000` retained text labels после первого mouse storm. Это не означает
“нарисовать все строки поверх одного viewport-а”: слой bake-ит весь набор,
а present-кадры draw-ят только видимый label-range. Так тест проверяет именно
retained buffer + culling, а не бессмысленную заливку GPU тысячами нечитаемых
надписей. Новые общие проверки добавляются как stages в этот же прогон. Узкие
диагностические сценарии допустимы только когда общий прогон мешает изолировать
другую проблему, как `order-cancel-lag` для задержки отображения отмены ордера.

## Idle-пол

Между `settle_live_chart` и `baseline` идёт стадия `idle_floor`: живой график открыт, но
никто его не трогает и present-pressure **выключен**. Это единственная стадия, которая
меряет приложение, когда его никто не заставляет работать. Живой BTC-фид законно гонит
собственный проход графика, поэтому ловит она не «ноль работы», а пробуждения GPUI-слоя без
ввода: broadcast на каждый тик, панель, перерисовывающаяся на ревизию, которая не менялась,
таймер, который никому не нужен.

Стоит она именно ТУТ, до `static_text_gap` и tool-окон, намеренно: у текстового слоя нет
пути отключения, а tool-окна не закрываются, так что idle-окно после них мерило бы простой
вместе с 10000 retained-меток и тремя открытыми окнами — и называло бы это полом.

Стадия пишет строку `[firetest] idle_floor avg/max …`. **avg и max для каждого счётчика
намеренно**: одиночная горячая секунда — затухание тинта прилетевшей новости, всплеск фида —
выглядит в одном только пике так же, как вечно крутящаяся панель, а лечатся они
противоположно. Пороги калиброваны по трём живым прогонам, а не назначены: news и detached
гейтятся только по среднему именно потому, что их пик в 115/s оказался легитимным
затуханием тинта.

**Эта калибровка устарела и ждёт пересмотра.** Тинт новости больше не идёт по vblank: он
перерисовывает панель на `crate::pulse::PULSE_TICK` (10 Гц), поэтому пик в 115/s из легитимного
превратился в дефект. Гейт по одному только среднему остался с тех времён — снимать его надо
прогоном, где `news_render` виден вместе с ненулевым `pulse_tick`.

## Рамка прибытия: стадия `arrival_flash`

Сразу за `idle_floor` и в том же холодном режиме идёт `arrival_flash`: FireTest сам зажигает
рамку прибытия на КАЖДОМ живом графике и держит её зажжённой всё окно (перезажигает каждые
2400 мс, потому что own-pass гасит её через 2600 мс). Затем гасит явно — `baseline` не должен
унаследовать догорающую рамку.

Стадия существует потому, что цену рамки нельзя вывести из чтения кода и нельзя дождаться:
живой детект не планируется, а прогон, которому не досталось ни одного прибытия, читается
ровно как «рамка бесплатна». Рамка живёт в own-pass и стоит **presents**, а present — оконный:
каждая соседняя канва в окне заново прогоняет свой проход, включая шейпинг текста. Штормовые
стадии этого увидеть не могут — они и так форсируют present.

Вердикт пишет строку `[firetest] arrival_flash idle->flash …`: `chart_present`, `chart_render`,
`shell_render` — на график; `bg_draw`, `grid_draw`, `base_bake`, `combo_bake`, `userdata_draw`,
`chart_gpu_prepare` — на кадр; CPU, `gpu_frame_ms` и `pulse_per_chart`. Каждая фаза делится на
СВОЁ число графиков: снимок стенда берётся в конце `idle_floor`, а живые детекты открывают графики
после него — из десяти записанных прогонов четыре выросли прямо внутри окна вспышки, один с 1 до 4.

**Потолка у этой стадии нет, и это замеренное решение, а не отложенное.** Десять прогонов
(2026-08-08) дали: вспышка стоит ~+10 present/s на горящий график — ровно те 10 Гц, которыми её
пейсит own-pass; `chart_render` при этом не двигается (медиана +0.15/s), а работа НА КАДР даже
падает (`bg_draw` 0.74 → 0.59 на present — лишние кадры переиспользуют базовый кэш). То есть
вспышка стоит кадров, а не работы в кадре, и потолок здесь гейтил бы константу пейсинга, а не
регрессию. Заслужит потолок другое: частота, отличная от 10 Гц на график, — и её `pulse_per_chart`
уже называет вслух.

Что стадия ВСЁ ЖЕ роняет — это себя саму: с включённой рамкой `pulse_per_chart` обязан быть
≥ 5/s (зажжённая рамка бампает счётчик 10 раз в секунду на канву), а с `MOON_ARRIVAL_FLASH=0` —
ровно ноль. Первое означает «сторона A действительно горела», второе — «сторона B правда
контроль»; без них прогон со сломанным хуком влился бы в выборку как «рамка ничего не стоит».

`chart_arrival_pulse` печатается и в строке `idle_floor` — там он говорит, случилось ли за
холодное окно настоящее прибытие. С 2026-08-08 на нём стоит гейт `7/s на график ПО СРЕДНЕМУ`:
застрявшая вспышка просит present каждые 100 мс, то есть держит 10/s бесконечно, а одно настоящее
прибытие — это 2.6 с из 5-секундного окна, 5.2/s, и оно законно. По среднему, а не по пику,
именно потому, что застрявшую от всплеска отличает длительность, а не частота: у застрявшей пик
совершенно нормальный. В десяти записанных прогонах реальное прибытие не попало в idle-окно ни
разу — счётчик был 0/0 во всех.

Там же затянут `idle_chart_render_avg_per_chart`: было 150/s, стало 30/s. Прежние 150 стояли
потому, что рамку подозревали в пробуждении этого счётчика, а источник пробуждения был «пока не
найден». Теперь он измерен напрямую — стадия `arrival_flash` не двигает `chart_render` вообще, —
а сами прогоны дали 4.4–5.9/s на график, то есть 150 висело в 25 раз выше всего наблюдаемого.

## Почему пороги нормируются

Один и тот же бинарь на одной машине даёт `chart_present` 119/s при одном чарте и 793/s при
пяти — сколько чартов вернёт сохранённая раскладка, решает не код. Поэтому потолки считаются
не в абсолютных величинах, а по природе счётчика:

- **own-pass чарта** (`bg_draw`, `grid_draw`, `combo_draw`, `userdata_draw`, `base_bake`,
  `combo_bake`, `orderbook_bake`, `chart_gpu_prepare`) — это работа ЗА КАДР, поэтому меряется
  отношением к `chart_present`: и дельтой над отношением baseline, и абсолютным потолком.
  По 13 записанным прогонам сырая дельта `bg_draw` гуляла 0…109, отношение — 0…0.13.
- **GPUI-рендеры вью** (`shell_render`, `orders_render`, `chart_render`) — НЕ работа за кадр,
  на present их делить нельзя (отношение гуляет 0.04…0.80 уже в baseline). У них абсолютный
  потолок; у `chart_render` он делится на число чартов, потому что панель рендерится на каждый
  чарт — но делится именно УРОВЕНЬ, не дельта.
- **Дельту делить на единицу стенда нельзя вообще**: baseline `c·b` минус шторм `c·b+s` уже
  сокращает `c`, и повторное деление просто делает большой стенд снисходительнее.

Все ТРИ шторма — чистый, static-text и `flash_storm` — проходят через одни и те же три функции
(`check_storm_input` — ввод дошёл и никого не разбудил, `check_storm` — работа на кадр,
`check_storm_load` — CPU/GPU/кадр) с одними потолками. Раньше их считали вместе, и цена 10000
меток приписывалась движению курсора.

### `flash_storm` — мышь ПОВЕРХ мигающей рамки

Отдельная стадия, потому что раньше эти два действия мерились раздельно: `arrival_flash` — рамка
без курсора, `mouse_storm` — курсор без рамки. В жизни они совпадают (детект открывает график,
пока пользователь уже водит мышью по стеку), и совпадать могут дороже суммы: и курсор, и рамка
просят у own-pass present и перестраивают readout. `flash_storm` гоняет тот же шторм, зажигая
рамку перед его стартом и перезажигая её всё окно, и сравнивается с тем же `baseline`.

Стоит он ПЕРЕД текстовым слоем намеренно: слой не выключается, и стадия после него мерила бы три
переменные вместо двух.

Вердикт пишет `[firetest] flash_storm clean->flash …` — сравнение с ЧИСТЫМ штормом, а не с
baseline: вопрос стадии «дороже ли мышь по мигающей рамке, чем мышь без неё». Замерено 2026-08-08
(2 прогона): `present` 117 → 117/s, `chart_render` 6 → 6/s, `entity` и `input_notify` 0 → 0,
CPU 3.0 → 2.7%, работа на кадр +0.02 (`bg_draw` 0.23 → 0.25). То есть **комбинация не дороже
суммы**: рамка добавляет в кадр один инстанс в readout-батч и не будит entity-путь.

Граница этого замера, которую надо знать: шторма идут под форсированным present, поэтому стадия
отвечает на «дороже ли КАДР», а не «больше ли КАДРОВ». Второе под мышью и не может вырасти —
кадры уже упираются в пейсер, который курсор насыщает сам; сколько кадров добавляет рамка
БЕЗ курсора, меряет холодная стадия `arrival_flash` (+10/s на график).

## Что тест обязан ловить

- на простое, без ввода, GPUI-слой не должен просыпаться: Shell/Orders/Assets/News,
  `backend_notify`, часы, компактор и order-sync держат измеренный потолок;
- cursor-only mousemove не должен будить `ChartPanel` entity path;
- команда UI в отсутствующее ядро должна возвращать runtime-ошибку, а не успешный no-op;
- tool-окна Settings/Strategies/Assets должны открываться реальными GPUI окнами и повторный open должен фокусировать существующее окно, а не создавать второе;
- Root-owned overlay слой должен открывать context menu, закрывать его при открытии dialog, заменять unique dialog по id, показывать notification и очищаться без висящих оверлеев;
- смена языка интерфейса должна живо доходить до глобальной локали rust-i18n и НЕ пересоздавать tool-окна (только redraw);
- выбор масштаба из toolbar-path должен дойти до активного chart state: `50%`, затем `20%`, затем `Auto`;
- opt-in place/cancel order test должен измерять путь `cancel_order` → входящий orders/server-log → `OrderLineStore` → chart userdata → GPU prepare → chart present/draw, и краснеть, если отменённый ордер дошёл до store, но график долго продолжает показывать старое состояние;
- cursor-only mousemove не должен делать `cx.notify()` для chart input/canvas;
- static text stress поверх графика не должен ломать mouse/input hot path и GPU frame budget;
- Shell и Orders GPUI render держат абсолютный потолок (замерено: 5.7/s средн., 11/s пик);
  `chart_render` — счётчик ГЛОБАЛЬНЫЙ, суммарный по всем открытым `ChartPanel`, поэтому его
  потолок делится на число чартов, иначе он врёт тем сильнее, чем больше чартов открыто;
- cursor-only mousemove не должен увеличивать частоту дорогих chart base draw/bake (`bg_draw`, `grid_draw`, `base_bake`, `combo_bake`, `orderbook_bake`) сверх baseline;
- `combo_draw_delta` остаётся строгим кроссплатформенным сигналом: cursor-only mousemove не должен добавлять дорогой combo draw сверх baseline. Если Metal/wgpu падают здесь, это не повод ослаблять FireTest, а сигнал довести retained/base-cache parity до уровня DX.
- CPU процесса не должен заметно расти от одной возни мышью;
- RAM не должна расти;
- на Windows дополнительно пишется process GPU `%` через PDH `GPU Engine`;
- на macOS системный process GPU `%` не подделывается: вместо него FireTest получает реальное Metal `GPUStartTime/GPUEndTime` completed command buffer и проверяет `gpu_frame_ms`;
- Linux mouse storm на X11 идёт через XTest (`DISPLAY`/`XAUTHORITY` реального тестового сеанса). Wayland без XWayland/XTest остаётся отдельной задачей: там нужен synthetic/platform test hook, `uinput` или compositor-specific runner.

## Почему есть high-present baseline

График на живом BTC сам по себе может часто печь base/combo из-за live-data и авто-Y. Поэтому FireTest не сравнивает mouse storm с “тихим” idle. Перед storm он включает такой же частый `gpu_canvas` present без курсора и использует максимум baseline-сэмплов как опору. Красный результат означает не “рынок был активен”, а “mousemove/readout добавили дорогую работу сверх уже горячего chart-present режима”.

## Критерий

Каждая стадия пишет лог вида:

```text
[firetest] stage=start
[firetest] stage=open_chart
[firetest] stage=wait_chart_probe
[firetest] stage=settle_live_chart
[firetest] stage=idle_floor
[firetest] stage=arrival_flash
[firetest] stage=baseline
[firetest] stage=mouse_storm
[firetest] stage=flash_storm
[firetest] stage=static_text_gap
[firetest] stage=static_text_warmup
[firetest] stage=static_text_storm
[firetest] stage=command_error_contract
[firetest] stage=tool_windows_open
[firetest] stage=tool_windows_verify_open
[firetest] stage=tool_windows_dedup
[firetest] stage=tool_windows_verify_dedup
[firetest] stage=root_overlay_contract
[firetest] stage=locale_switch
[firetest] stage=locale_switch_verify
[firetest] stage=price_scale_50
[firetest] stage=price_scale_20
[firetest] stage=price_scale_auto
[firetest] stage=price_scale_verify_auto
[firetest] stage=order_cancel_lag
[firetest] stage=cooldown
```

Успех пишет `firetest.log` строку:

```text
[firetest] result=PASS FIRETEST PASS ...
```

Ошибка пишет `result=FAIL FIRETEST FAIL ... reasons=...` или
`result=FAIL FIRETEST FAIL reason=...` и завершает процесс кодом `2`.

Тест специально краснеет от регрессий вида “на mousemove кто-то снова сделал top-down render, notify, тяжёлый запрос, аллокационный render path или дорогой GPU frame”. Скриншот не является критерием этого теста; FireTest проверяет поведение и нагрузку.
