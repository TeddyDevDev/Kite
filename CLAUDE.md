# Kite — normas del proyecto

Kite es un ayudante para League of Legends **ultra ligero, rápido y estable**, con estilo limpio (oscuro, tipo Blitz pero sin ruido).
Stack: **Rust + Slint** (renderer por software, sin navegador/WebView).

## 🚫 NORMAS ANTI-BAN (Vanguard / políticas de Riot) — NO NEGOCIABLES

Antes de añadir cualquier función, comprobar que cumple TODO esto. Si hay duda, NO se hace.

### Solo se permite usar
- **LCU API** (cliente, `https://127.0.0.1:<puerto>` con credenciales del `lockfile`): REST + WebSocket.
- **Live Client Data API** (`https://127.0.0.1:2999/liveclientdata/...`), solo para datos PROPIOS.
- **Data Dragon / CommunityDragon** y la **API web oficial de Riot** (con API key personal).

### Prohibido siempre
- Leer o escribir memoria del juego, inyectar DLLs o código en el cliente o en el juego (nada tipo Pengu Loader).
- Modificar archivos del juego o del cliente.
- Automatizar inputs dentro de la partida (scripts, macros).
- **Revelar nombres del lobby / desanonimizar jugadores**: en champ select solo se lee `localPlayerCellId` y TU celda de `myTeam`. Nunca se consultan nombres, PUUID ni historiales de otros jugadores en champ select.
- Durante la partida (incluida la pantalla de carga): nada sobre los rivales. Ni sus campeones o jugadas, ni estadísticas agregadas del lobby, ni predicciones.
- Timers de ultis o hechizos enemigos (ni automáticos ni manuales), avisos de picos de poder ("X llega a nivel 6") ni alertas tipo "ve aquí ahora".
- Mostrar winrates de cosas que Riot prohíbe (por ejemplo, Legends/Augments de TFT).

### Dónde va cada función
- **Champ select**: builds, runas, hechizos y counters de TU línea. OK.
- **En partida**: como máximo, TU propia build y orden de habilidades.
- **Post-partida**: análisis y aprendizaje. OK.

## ⚡ Normas de rendimiento
- Eventos por el WebSocket de la LCU, **nunca sondeo** del cliente. Sin LoL abierto solo se comprueba el lockfile cada 3 s.
- Sin cuentas, anuncios ni telemetría. La única salida a internet es descargar datos del parche (cacheados en local).
- Todo lo que se pueda, en caché local (SQLite) para que champ select sea instantáneo.
- Si el cliente se cierra o se actualiza, se reconecta solo. Nunca `unwrap()` en rutas de red o de datos externos.
- Objetivo: menos de 35 MB de RAM, 0% de CPU en reposo y un .exe de menos de 15 MB.

## Estructura
- `src/main.rs`: ventana y puente con la interfaz.
- `src/lcu.rs`: conexión con el cliente (lockfile, TLS local, REST mínimo, WebSocket), runas recomendadas (`/lol-perks/v1/recommended-pages/...`) e importación.
- `src/config.rs`: ajustes en `%APPDATA%\Kite\config.json`.
- `src/ddragon.rs`: Data Dragon (versión del parche, nombres de objetos en inglés, iconos) cacheado en `%APPDATA%\Kite\ddragon`.
- `src/opgg.rs`: cliente del MCP oficial de OP.GG (`https://mcp-api.op.gg/mcp`), fuente de builds, counters y sinergias. Solo HTTP+JSON; no se instala nada de OP.GG. Respuestas en un formato de texto propio (`class X: campos` + `X(valores)`) que `parse_notation` convierte a JSON. Se cachea 12 h por campeón y línea en `%APPDATA%\Kite\opgg`. Nombres de campeón en MAYÚSCULAS_CON_GUIONES (LEE_SIN); se valida que `data.summary.id` sea el campeón pedido. `testdata/opgg_ahri_mid.txt` es una respuesta real para los tests.
- `src/stats.rs`: tipos de build que consume la interfaz.
- `ui/app.slint`: interfaz (cabecera con estado → héroe con arte del campeón y rol → runas → build → ajustes).
- `assets/`: logo (monograma K) como `kite.ico` (icono del .exe, vía `build.rs` + winresource) y `kite-256.png` (icono de la ventana).
- `instalar.bat`: compila en release, copia `Kite.exe` y crea accesos directos (solo hace falta una vez).
- `src/updater.rs`: autoactualización. Si el código es más nuevo que `Kite.exe`, compila sin consola, renombra el .exe en marcha a `Kite.old.exe`, copia el nuevo y reinicia. Errores de compilación en `update.log`.

## Importación de runas
- Runas: solo las de OP.GG (no las recomendadas por Riot), mostradas con iconos sacados del propio cliente.
- Orden al importar: 1) si existe la página "Kite: …", se modifica; 2) si no, se crea; 3) si no hay hueco, **se modifica la página actual** (decisión del usuario). Nunca se crean páginas de más.
- La importación automática se hace una sola vez por campeón, al bloquear.

## Build en partida
- Kite muestra TU build (principal, botas, inicio, habilidades y situacionales) también durante la partida.
- Los situacionales son una lista fija por campeón y línea, sacada de OP.GG. **Nunca dependen de los rivales de la partida en curso.**
- Los nombres de objetos se muestran en inglés (idioma de juego del usuario). Destello siempre en la F (segundo hechizo).

## Comandos
- `cargo run`: abrir con ventana.
- `cargo run -- --probar-opgg`: prueba OP.GG y guarda las respuestas en `.\probe\`.
- `cargo run -- --debug`: sin ventana; imprime los eventos del cliente en la consola.
- `cargo build --release`: genera el .exe final en `target/release/kite.exe`.
- Variable `KITE_LOCKFILE`: ruta manual al lockfile si LoL está en otra carpeta.
