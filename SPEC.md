# SVC-клиент для Horizon — техническая спецификация

Статус: этап 1–2 (исследование и спецификация). Дата: 2026-11-19.

## 0. Важные поправки к постановке задачи

Проверено по исходникам, а не по памяти:

1. **`.hnmod` больше не загружается.** Лаунчеры Horizon начиная с `0.2.70` читают только
   `.hmod`; `.hnmod` — старый формат, который показывается в списке модов только с кнопкой
   «удалить». Подтверждение: `horizon/launcher/src/native_status.rs:247-256, 786-866`.
   Поэтому проект собирается в **`svc_client-0.1.0.hmod`**.
2. **`StaticSoundPacket` в сетевом протоколе SVC нет.** По проводу идут:
   `MicPacket` (1), `PlayerSoundPacket` (2), `GroupSoundPacket` (3), `LocationSoundPacket` (4).
   `StaticSoundPacket` существует только как plugin-API-обёртка (`StaticSoundPacketImpl`) и на
   провод не попадает. Для статичного звука используется `LocationSoundPacket`.
3. **`PROTOCOL_VERSION` как имени в актуальном коде нет.** Есть
   `BuildConstants.COMPATIBILITY_VERSION` из `gradle.properties`.
   - стабильная ветка `2.6.x/26.3` (коммит `2e56198016e02fd99139d37f382b7155c5bd63f5`,
     версия `2.6.28+26.3`) — **compatibility version 20**;
   - ветка `26.3` (коммит `137d62f6c93a3d02b50a1b67964814e6c24a16d4`, версия
     `2.7.0-pre1+26.3`) — **compatibility version 21**.
   `2.7` принимает клиентов версии 20 только при включённом системном свойстве
   `-Dvoicechat.legacy` (`LegacyCompatibility.isEnabled()`), по умолчанию — нет.
   **MVP целится в версию 20**, потому что это текущий стабильный протокол. Rust-relay
   `horizon-mods/native/voicechat-server` использует ту же раскладку как референс, но живая
   совместимость именно с этим relay не проверялась.
4. **Расширение пакета — zip.** Внутри: `horizon-mod.toml`, `Cargo.toml`, `src/`, ресурсы.
   Никакого каталога с id мода внутри архива нет.

## 1. Проверенные версии и ревизии

| Что | Значение | Источник |
|---|---|---|
| Игра | Minecraft Java Edition **26.3** | `horizon/README.md`, `sdk.json` |
| Horizon HEAD на дату проверки | `716961b24de29a71ed6ecb99412cbe3a28ff3552` | `git log` |
| **Целевой build (pin)** | **`dd67b76ff23e28c6d8e21859df600d31572f81a0`** | SDK-релиз `sdk-dd67b76…`, Nightly 2026-11-19 |
| Rust toolchain SDK | **1.99.1** | `sdk.json` (`toolchain.rust`) |
| `horizon` crate rust-version | 1.95, edition 2024 | `horizon/Cargo.toml` |
| Mod API | **0.2** | `sdk.json` (`content_api`) |
| horizon-mixins для pin | `e59da9094f0c2fdda6c627fe9eff22dec6a906f0` | `sdk.json` (`mixins.rev`) |
| SVC стабильный | 2.6.28+26.3, compatibility **20** | ветка `2.6.x/26.3` |
| SVC dev | 2.7.0-pre1+26.3, compatibility **21** | ветка `26.3` |
| Opus | 48 kHz, mono, 20 ms = **960 samples**, max payload **1275 B** | `AudioUtils.java` |
| UDP-порт SVC по умолчанию | **24454** | `ServerConfig.java` |
| SVC license | «All Rights Reserved» | `svc26/license` |
| Лицензия проекта | **GPL-3.0-or-later** | `LICENSE` |

Ссылки на локальные клоны (этап исследования):
`/tmp/svc-port-refs/horizon`, `/tmp/svc-port-refs/horizon-sdk` (worktree pin),
`/tmp/svc-port-refs/svc26`, `/tmp/svc-port-refs/simple-voice-chat`,
`/tmp/svc-port-refs/horizon-mods`.

## 2. Точки интеграции Horizon

Проверено по сгенерированному API из SDK `dd67b76`, а не по догадкам.

### 2.1 Манифест и сборка

`horizon-mod.toml`:

```toml
id = "svc_client"
api = "0.2"
side = "client"
capabilities = ["registries"]   # как в шаблоне cargo horizon-mod

[native]
game = "26.3"
horizon = "dd67b76ff23e28c6d8e21859df600d31572f81a0"
mixins = "e59da9094f0c2fdda6c627fe9eff22dec6a906f0"
```

`Cargo.toml` обязан ссылаться на SDK **только** так:

```toml
[dependencies]
horizon-mod-api = { path = ".horizon/sdk/api/crates/horizon-mod-api" }
```

Другие источники (`git`, `version`, другой `path`) лаунчер отвергает
(`docs/modding/src/reference/sdk/03-in-a-project.md`). Прочие crates.io-зависимости
разрешены, но должны быть зафиксированы через `horizon-launcher native-lock .`
(комментарий в `cargo-horizon-mod/template/native/api.toml`).

### 2.2 Хуки

| Хук | Точная сигнатура (SDK) | Назначение |
|---|---|---|
| `register` | `fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext)` | бинды и настройки |
| `client_player_logging_in` | `fn …(&self, host: &mut dyn ClientHost, event: &mut ClientPlayerLoggingInEvent)` | новый вход на сервер: запросить secret |
| `client_player_logging_out` | `fn …(&self, host: &mut dyn ClientHost, event: &mut ClientPlayerLoggingOutEvent)` | выход: остановить сессию |
| `client_disconnecting` | `fn …(&self, host: &dyn ClientHost, event: &mut ClientDisconnectingEvent)` | разрыв: остановить сессию |
| `client_packet` | `fn …(&self, host: &mut dyn ClientHost, event: &mut ClientPacketEvent<'_>)` | приём **любого** play-пакета; `event.packet.custom_payload: Option<ClientPayload>` даёт `channel: String` и `data: Vec<u8>` |
| `client_tick_post` | `fn …(&self, host: &mut dyn ClientHost, event: &mut ClientTickPostEvent)` | раз в клиентский тик: отправить `request_secret`, обновить listener, PTT, очередь control-пакетов |
| `gui_render_post` | `fn …(&self, host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>)` | HUD: `event.batch: &mut dyn DrawContext`, `size()`, `fill()`, `text()`, `gui_hidden`, `focused` |
| `input_key` | `fn …(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>)` | опционально; для PTT используется не он, а зарегистрированный бинд |

`client_packet` приходит **до** обработки пакета игрой и видит custom payload целиком, включая
неизвестные ванильному клиенту каналы. Это подтверждено тестом
`horizon-client/tests/it/mod_api_hooks.rs::client_packet_hands_a_mod_the_type_and_custom_payload`,
где в качестве примера используется ровно `voicechat:secret`.

### 2.3 Отправка Plugin Channel

Проверено по движку: **raw-каналы (`play_raw`) — это политика сервера**. На клиенте
`send_payload_to_server` для raw-канала возвращает `WrongDirection`
(`horizon-client/src/raw_payloads.rs:345-352`), а таблица raw на клиенте вообще пустая
(`raw_payloads.rs:138-145`). Поэтому клиент отправляет SVC-сообщения как обычный ванильный
`minecraft:custom_payload` через `ClientHost::send_packet`:

```rust
let mut body = Writer::new();
body.string("voicechat:request_secret"); // Ident + VarInt length
body.buf.extend_from_slice(&20i32.to_be_bytes()); // application bytes as-is
host.send_packet(ServerboundPlayPacket::new(
    Identifier::parse("minecraft:custom_payload").unwrap(),
    body.into_vec(),
))
```

Это ровно тот путь, который проверяет сам движок в
`horizon-client/src/game/modding/native_net.rs:100-107` — с каналом
`voicechat:request_secret`. `CustomPayload::write` пишет `Ident` и затем тело без
дополнительного префикса; `PayloadCodec::read` читает `Ident` и `rest()`.

Входящие `voicechat:*` ловятся в `client_packet`: канал и сырые байты приходят в
`event.packet.custom_payload` (`ClientPayload { channel, data }`). Объявлять канал для
приёма не нужно.

Отправлять `voicechat:*` через `send_payload_to_server` нельзя: это либо
`WrongDirection`, либо `UnknownPayload`, потому что сервер не обязан объявлять эти каналы
в Horizon-манифесте.

### 2.4 Чтение состояния игры

| Нужно | API |
|---|---|
| адрес сервера, к которому подключены | `host.client_session().client_server()?.address: Option<String>` (`ClientServer`) |
| локальный игрок | `host.local_player() -> Option<PlayerRef>` |
| игрок по UUID | `host.player(uuid) -> Option<PlayerRef>` |
| позиция/глаза сущности | `EntityRef::position(host)`, `EntityRef::eye_position(host)` |
| имя игрока | `EntityRef::name(host)` |
| камера (позиция, yaw/pitch) | `host.camera() -> Option<Camera>` (позиция — глаз камеры) |
| состояние бинда | `host.is_down("svc_client", "push_to_talk")` |
| клик по бинду | `host.consume_click("svc_client", "mute")` |
| настройки | `host.setting_bool/choice/f64("key")`, `host.set_setting(...)` |
| чат/лог | `log::info!` (крейт `log` уже в SDK) |

### 2.4.1 Поиск других игроков на клиенте

Проверено по реализации, а не по документации:

- `PlayersHost::player(uuid)` на клиенте находит только локального игрока и
  LAN-гостей; для чужих игроков внешнего сервера `player_lookup` возвращает `None`
  (`horizon-client/src/game/modding/native_server.rs:734-751, 819-861`).
- `EntitiesServerHost::entity_by_uuid` явно возвращает `None` на клиенте
  (`native_entity_refs.rs:142-145`).
- `EntityRef::uuid(host)` сгенерирован с `&mut dyn ServerHost`, то есть на клиенте
  недоступен.

Поэтому позиция говорящего находится так:

1. `ClientSession::client_tab_list()` даёт UUID → profile name (работает на клиенте).
2. `EntityRef::name(host)` (`EntitiesHost`, обе стороны) даёт имя сущности; для
   игрока это profile name, если сервер не поставил custom name.
3. Пока игрок не найден, раз в тик сканируется ограниченный диапазон entity id
   (`host.entity(id)` + `entity.name(host)`), по 4096 id за тик, максимум 65535.
   Найденный `EntityRef` кэшируется и проверяется по имени; при смене измерения
   (`client_level_load`) кэш сбрасывается. После полного прохода без результата
   повторный скан откладывается на 5 секунд, чтобы custom-name игрок не вызывал
   сканирование на каждом звуковом пакете.

Это компромисс: публичный API не даёт UUID→EntityRef на клиенте, а лезть в
`game::remote` через нативный mixin (как делают сторонние SVC-моды) — значит
терять проверяемую сборку и привязываться к внутренностям. Ограничения обхода: игрок, скрытый из tab-list (`player info remove`), не
находится;
игрок с custom name не будет найден; entity id выше 65535 не находится; скан
стоит несколько тысяч lookup'ов в тик, пока говорящий не найден; однофамилец-моб
теоретически может совпасть по имени. Для точного UUID→EntityRef нужен нативный
mixin в `game::remote`, как у сторонних SVC-модов, но это ломает проверяемую
сборку и привязывает мод к внутренностям конкретного билда.

### 2.5 Бинды и настройки

`Registrar::register_key_mapping(KeyRegistration::new("push_to_talk", "key.keyboard.caps.lock"))`.
Поля `KeyRegistration`: `id`, `label`, `key`, `modifiers`, `category`, `context`; есть
конструктор `new` и `context(...)`.

`Registrar::register_setting(SettingSpec::toggle/choice/range/color(...))` — только client,
появляются в **Options > Mods > SVC**. Чтение — типизированное, без парсинга строк.

### 2.6 HUD

`gui_render_post` даёт `DrawContext`:

- `size() -> [f32; 2]` — GUI-пиксели;
- `fill([x, y, w, h], 0xAARRGGBB)`;
- `text(&str, [x, y], argb, shadow)`;
- `text_width(&str)`, `push_clip/pop_clip`, `gui_nine_slice`, `sprite`, `item`.

Этого достаточно для индикатора микрофона и списка говорящих.

### 2.7 Ограничения API (учтено в архитектуре)

- `client_packet` **нельзя** отменить или подменить пакет; SVC-пакеты и так игнорируются
  ванильным клиентом Horizon, поэтому достаточно наблюдать.
- `client_packet_send` тоже только наблюдает/отменяет, но не вставляет пакеты. Вставка —
  только через `send_packet` с `minecraft:custom_payload`.
- Configuration-фаза в `client_packet` не видна. SVC шлёт `request_secret` уже в play
  (после `onJoinWorld` в Java-клиенте), так что это не мешает.
- `send_packet` работает в play-фазе; configuration-фаза для SVC не нужна.
- Потоки и сокеты API не запрещает: нативный мод не в песочнице и имеет права игрока.
  UDP, cpal и Opus живут в собственных потоках мода.

## 3. Протокол SVC (compatibility 20, стабильный 2.6.x)

Все числа — big-endian. UUID — 16 байт (два `long`: most significant, least significant).
Строки и байтовые массивы — `VarInt` длина + UTF-8/байты. `bool` — 1 байт 0/1.

### 3.1 TCP Plugin Channels (через игру)

| Канал | Направление | Поля |
|---|---|---|
| `voicechat:request_secret` | C→S | `i32 compatibilityVersion` (4 байта) |
| `voicechat:secret` | S→C | `[u8;16] secret`, `i32 port`, `UUID player`, `u8 codec` (0 VOIP, 1 AUDIO, 2 RESTRICTED_LOWDELAY), `i32 mtu`, `f64 distance`, `i32 keepAliveMs`, `bool groups`, `String voiceHost` (UTF, max 32767), `bool allowRecording` |
| `voicechat:update_state` | C→S | `bool disabled` (в v20; в v21 канал `client_state` и `bool muted, bool disabled`) |
| `voicechat:state` | S→C | v20: `bool disabled`, `bool disconnected`, `UUID`, `String name`, `bool hasGroup`, `[UUID group]` |
| `voicechat:states` | S→C | `i32 count`, затем `count` × state |
| `voicechat:remove_state` | S→C | `UUID` |
| `voicechat:add_group` | S→C | `UUID id`, `String name`, `bool hasPassword`, `bool persistent`, `bool hidden`, `i16 type` (0 normal, 1 open, 2 isolated) |
| `voicechat:remove_group` | S→C | `UUID` |
| `voicechat:create_group` | C→S | `String name`, `bool hasPassword`, `[String password]`, `i16 type` |
| `voicechat:set_group` | C→S | `UUID group`, `bool hasPassword`, `[String password]` |
| `voicechat:leave_group` | C→S | пусто |
| `voicechat:joined_group` | S→C | `bool hasGroup`, `[UUID group]`, `bool wrongPassword` |

Для MVP реализуются `request_secret`, `secret`, `state`, `states`; групповая логика —
следующий этап. Лимиты имён групп: 24 символа, пароль 256 (в Horizon-сервере — 24).

### 3.2 UDP: внешний формат

Клиент → сервер (v20):

```
0xFF | UUID player (16) | VarInt len | sealed[len]
```

Сервер → клиент (v20 и v21):

```
0xFF | VarInt len | sealed[len]
```

`sealed = IV(12) | AES-128-GCM(ciphertext + tag(16))`.

Открытый текст: `u8 packetType | body`.

Важно: в v21 клиент→сервер вместо `UUID player` несёт `UUID connectionId`, а `secret`
дополнительно содержит `connectionId`. Именно поэтому v20-реализация не может просто
«переключиться» на v21 без учёта формата.

### 3.3 UDP: типы пакетов

| ID | Имя | Направление | Тело |
|---|---|---|---|
| `0x01` | `MicPacket` | C→S | `byte[] data` (`VarInt len` + Opus, ≤1275), `i64 sequence`, `bool whispering` |
| `0x02` | `PlayerSoundPacket` | S→C | `UUID channelId`, `UUID sender`, `byte[] data`, `i64 sequence`, `f32 distance`, `u8 flags` (bit0 whisper, bit1 category), `[String category (≤16)]` |
| `0x03` | `GroupSoundPacket` | S→C | `UUID channelId`, `UUID sender`, `byte[] data`, `i64 sequence`, `u8 flags`, `[String category]` |
| `0x04` | `LocationSoundPacket` | S→C | `UUID channelId`, `UUID sender`, `f64 x,y,z`, `byte[] data`, `i64 sequence`, `f32 distance`, `u8 flags`, `[String category]` |
| `0x05` | `AuthenticatePacket` | C→S | v20: `UUID player`, `[u8;16] secret`; v21: пусто |
| `0x06` | `AuthenticateAckPacket` | S→C | v20: пусто; v21: `[u8;16] nonce` |
| `0x07` | `PingPacket` | оба | `UUID id`, `i64 timestamp` |
| `0x08` | `KeepAlivePacket` | оба | пусто |
| `0x09` | `ConnectionCheckPacket` | C→S | v20: пусто; v21: `[u8;16] nonce` |
| `0x0A` | `ConnectionCheckAckPacket` | S→C | пусто |

Ограничение чтения зашифрованного тела — 2048 байт; Opus — 1275 байт.

### 3.4 Криптография

- AES-128-GCM, ключ — 16-байтовый `secret` из `voicechat:secret`.
- IV — 12 случайных байт на **каждый** пакет, передаётся первым.
- Тег — 16 байт, дописывается GCM.
- Никакого CBC в актуальном протоколе нет; CBC (`AES/CBC/PKCS5Padding`) был только в
  legacy v18 (`LegacyProtocolV18`) и для цели не нужен.
- TCP-каналы не шифруются приложением: их защищает транспорт Minecraft.

### 3.5 Рукопожатие (v20)

1. Клиент отправляет `voicechat:request_secret` с `20`.
2. Сервер присылает `voicechat:secret`.
3. Клиент открывает UDP и раз в 1 с шлёт `Authenticate` (5) с `player UUID + secret`,
   пока не получит `AuthenticateAck` (6).
4. После ack шлёт `ConnectionCheck` (9) раз в 1 с, пока не получит
   `ConnectionCheckAck` (10).
5. После этого — `Connected`. Сервер шлёт `KeepAlive` каждые `keepAlive` мс (обычно 1000),
   клиент отвечает тем же `KeepAlive`. Нет keepalive `10 × keepAlive` — разрыв.
6. `PingPacket` от сервера возвращается как есть (pong).

### 3.6 Версия 21 (2.7) — что изменится

- `secret` начинается с `secret(16)`, затем `UUID connectionId`, затем остальные поля.
- `Authenticate` пустой, `AuthenticateAck` несёт 16-байтовый nonce, `ConnectionCheck`
  возвращает этот nonce.
- Внешний UDP-заголовок клиента несёт `connectionId`, а не `player UUID`.
- `voicechat:update_state` заменён на `voicechat:client_state` с двумя bool.
- `PlayerState` — байт флагов (muted/forceMuted/disabled/forceDisabled/disconnected/hasGroup),
  а не два bool, как в v20.
- `mtu` зажимается в 512..2048, добавлен `allowRecording` (в v20 он тоже есть в конце).

## 4. Аудио-пайплайн

### 4.1 Захват

- cpal input stream, цель — 48 kHz, mono, 16-bit/f32.
- Если устройство не даёт 48 kHz/mono — берём default config и линейно ресемплим в
  48 kHz mono (состояние ресемплера в потоке микрофона).
- Кадр Opus: 960 сэмплов (20 мс), `Application::Voip`, максимум 1275 байт.
- Усиление микрофона: multiply + clamp.
- PTT: `host.is_down("svc_client", "push_to_talk")` в `client_tick_post` → atomic.
- Voice activation: peak dBFS кадра > порога; hangover 250 мс.
- `enabled = false` или mute → кадры не отправляются; при переходе в тишину отправляется
  `MicPacket` с пустым `data` (stop), как в SVC.

### 4.2 Приём и декодирование

- Один `opus::Decoder` на отправителя (моно, 48 kHz).
- `PlayerSoundPacket` → позиция отправителя = `player.entity().eye_position(host)`.
- `GroupSoundPacket` → статичный (центр), без панорамы.
- `LocationSoundPacket` → позиция из пакета.
- Порядок: `sequence`; дубликаты и старые пакеты отбрасываются; небольшой jitter-буфер
  (2–3 кадра), при переполнении старые кадры выбрасываются.

### 4.3 Затухание и панорама (порт формул SVC)

Дистанция:

```
d = min(|source - listener|, maxDistance)
gain = 1 - d / maxDistance
```

Если `d > maxDistance + 1` — звук не слышен.

Панорама (SVC `PositionalAudioUtils.getStereoVolume`):

```
dir = normalize(source - listener)
diffAngle = degrees(atan2(-dir.x, dir.z))
angle = normalizeAngle(diffAngle - yaw)
vertical = min(|listener.y - source.y| / 32, 1)
rot = angle / 180
perc = fold(rot)          // ±0.5 → край, дальше заворачивается
perc *= (1 - vertical)
left  = perc < 0 ? |perc * 1.4| + 0.3 : 0.3
right = perc >= 0 ? perc * 1.4 + 0.3 : 0.3
fill = 1 - max(left, right)
left += fill; right += fill
// constant-power:
power = sqrt(left² + right²)
left /= power; right /= power
```

Итог: `sample * gain * left/right`, затем clipping в i16. Это воспроизводит поведение
SVC настолько, насколько это возможно без OpenAL.

### 4.4 Вывод

- cpal output stream, stereo; предпочтительно 48 kHz, иначе default + линейный ресемплер.
- Микшер: `HashMap<UUID, Source>`; каждый `Source` — очередь decoded i16, позиция,
  `maxDistance`, время последнего пакета.
- Callback вынимает по одному сэмплу на источник, суммирует, клиппит.
- Потокобезопасность: `Arc<Mutex<Mixer>>`; в MVP допускается короткий lock в callback,
  это документированный риск (в идеале — lock-free ring buffer).

## 5. Проверенные крейты

Собрано `cargo check`, `cargo test` и `cargo build --release` на Linux x86-64,
Rust 1.99.1. SDK взят из релиза `sdk-dd67b76…`: SHA-256 `api-source.tar.xz`
совпал с `sdk.json` (`4981ce83…c0efa`).

| Крейт | Версия | Проверка |
|---|---|---|
| `cpal` | 0.18.2 | компилируется; на Linux нужен `libasound2-dev` (ALSA 1.2.16.1) + pkg-config; PipeWire 1.6.11 работает через ALSA-совместимость. API: `build_output_stream(config, cb, err, None)`, `SampleRate = u32`, `BufferSize::Default` |
| `opus` | 0.4.0 | компилируется; зависит от `opusic-sys` 0.7.7 с **feature `bundled` по умолчанию**, то есть libopus собирается через CMake и статически входит в мод. Системный `libopus` в рантайме не нужен, но `cmake` нужен на машине сборки. API: `Encoder::new(48000, Channels::Mono, Application::Voip)`, `Decoder::new(...)`, `encode(&[i16;960], &mut [u8;1275])` |
| `aes-gcm` | 0.11.1 | компилируется; `Aes128Gcm::new_from_slice(&key)`, `encrypt(nonce, plain)`, тег 16 байт |
| `rand` | 0.10.4 | компилируется; `rand::random::<[u8;12]>()` для IV |
| `log` | 0.4 | уже в SDK |

Альтернативы, которые **не** выбраны:

- `audiopus` 0.3.0-rc.0 — лишний слой, версия RC; `opus` стабильнее.
- `ring` — используется в Horizon-сервере, но тянет сборочные зависимости; `aes-gcm`
  проще и уже проверен.
- `tokio` — не нужен: один UDP-сокет и один поток с `read_timeout` проще и не создаёт
  конфликтов с рантаймом игры.
- `rodio`/`kira` — лишняя абстракция; нужен ручной 3D-микшер.

## 6. Архитектура проекта

```
src/
  lib.rs                 — Hooks, регистрация, жизненный цикл
  protocol/
    mod.rs               — типы пакетов
    buf.rs               — Reader/Writer (BE, VarInt, UUID, UTF)
    tcp.rs               — request_secret / secret / state / states
    udp.rs               — NetworkMessage, packet ids, внешние датаграммы
    crypto.rs            — AES-128-GCM
  session.rs             — состояние сессии, поток UDP, handshake, keepalive
  audio/
    mod.rs               — запуск движка
    capture.rs           — cpal input + ресемплер + Opus encode
    playback.rs          — cpal output + микшер
    mixer.rs             — очередь источников, 3D-микс
    spatial.rs           — формулы затухания/панорамы
  hud.rs                 — DrawContext-оверлей
  settings.rs            — SettingSpec + чтение
  state.rs               — общее состояние (Mutex), статус, speaking map
```

Потоки:

1. **Main/game thread** — хуки, парсинг TCP, отправка `request_secret`, обновление
   listener-снимка, PTT/настройки, HUD.
2. **UDP thread** (на сессию) — сокет, handshake, keepalive, приём sound-пакетов,
   декодирование Opus, push в микшер.
3. **Mic thread** — cpal input stream + накопление 960 сэмплов + encode + отправка
   `MicPacket` через канал в UDP thread.
4. **Output thread** — cpal output stream + микшер.

Никаких `unsafe` блоков не планируется.

## 7. Что проверено, а что нет

Проверено:

- исходники Horizon и SVC прочитаны в клонах;
- SDK `dd67b76` скачан, `cargo check -p horizon-mod-api` проходит;
- `cpal`, `opus` (bundled libopus), `aes-gcm`, `rand` собираются;
- `cargo test` — 35 тестов, включая полный UDP-handshake против локального mock-сервера;
- точные сигнатуры хуков и host-методов взяты из сгенерированного API;
- формат пакетов сверен с Java v20 и с Rust-сервером Horizon.

Не проверено (нужен живой сервер/клиент):

- полная сборка игрового движка Horizon (нужны ресурсы Minecraft 26.3 и лаунчер);
  проверена сборка против pinned SDK и release-сборка самого мода;
- реальный handshake с Paper/Fabric SVC 2.6.28;
- захват/вывод звука на конкретном устройстве;
- поведение в бою с другими модами, меняющими HUD/аудио;
- v21 (2.7) — не реализуется в MVP;
- LAN-мир, опубликованный из клиента (`integrated_local = true`), считается
  singleplayer и голосовой сервер не поднимается; SVC в таком режиме может работать,
  но его порт не доступен через публичный API;
- `ClientServer.address` — адрес, введённый игроком; если сервер доступен только
  через SRV-запись без A/AAAA, UDP-хост может не разрешиться. SVC Java берёт
  resolved socket address; публичный API Horizon его не отдаёт.

## 8. Команды проверки

```sh
cp -r /tmp/svc-port-refs/sdk .horizon/sdk/api   # локальная копия pinned SDK
cargo check --locked
cargo test
cargo build --release
cargo horizon-mod check target/horizon-mod/svc_client-0.1.0.hmod
```

Официальный чекер из pinned-ревизии принял пакет:
`svc_client 0.1.0 (native-mod, format native-1/dd67b76…/e59da90…, for 26.3)`, exit 0.

Пакет: `target/horizon-mod/svc_client-0.1.0.hmod` (zip: манифест, Cargo.toml,
Cargo.lock, src/, assets/, README/LICENSE; без `target/`, `.horizon/`, `.git/`).
