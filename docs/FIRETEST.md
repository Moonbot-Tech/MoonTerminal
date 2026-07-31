# FireTest

Дата актуализации: 2026-07-25.

FireTest — встроенный debug/test scenario runner для поиска дорогих UI-ошибок в горячем chart path.

## Запуск Windows

```powershell
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /d /s /c "`"$vcvars`" && cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc"
Remove-Item -ErrorAction SilentlyContinue firetest.log, render_diag.log
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

Один порог стоит особняком. `idle_chart_render_avg` ≤ 150 — это **зафиксированная, а не
одобренная** цена: без ввода и без форсированного present `ChartPanel::render` крутится
41–93/s, то есть чаще, чем под мышиным штормом, где тот же счётчик держат на 10/s, потому
что будить entity на движении курсора считается дефектом. Что будит его на простое — пока
не найдено, поэтому порог поставлен ловить УДВОЕНИЕ этой цены, а не благословлять её.
Опустить, когда источник пробуждений найдут и починят.

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
- Shell/Orders/Chart GPUI render не должны улетать в сотни render/s;
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
[firetest] stage=baseline
[firetest] stage=mouse_storm
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
