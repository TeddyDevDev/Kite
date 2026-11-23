# Kite

Ayudante para League of Legends: ligero, rápido y limpio. Solo usa APIs oficiales, sin anuncios ni telemetría.

## Descargar (lo más fácil)
Descarga **Kite.exe** de la última versión en [Releases](https://github.com/TeddyDevDev/Kite/releases/latest), ponlo en cualquier carpeta y ábrelo con doble clic. No hace falta instalar nada más.

> Windows puede avisar con "Windows protegió su PC" porque el .exe no está firmado: pulsa **Más información → Ejecutar de todas formas**.

## Compilar desde el código
## Requisitos (Windows)
1. Rust: https://rustup.rs (instalador `rustup-init.exe`, opciones por defecto).
2. Las Build Tools de Visual Studio con "Desarrollo para el escritorio con C++" (rustup te lo ofrece al instalar).

## Instalar (abrir con doble clic, sin consola)
Doble clic en `instalar.bat`: compila la versión final, crea `Kite.exe` y pone accesos directos en el escritorio y en el menú Inicio.
Solo hace falta una vez: a partir de ahí, al abrir Kite se actualiza solo si hay cambios en la carpeta (y si los hay con Kite abierto, sale un botón "Actualizar").

## Ejecutar en modo desarrollo
```
cd E:\kite
cargo run              # con ventana
cargo run -- --debug   # sin ventana, muestra los eventos del cliente
cargo build --release  # .exe final en target\release\kite.exe
```

Abre el cliente de LoL y Kite se conecta solo.

## Builds
Salen de OP.GG (API oficial, sin key). Para probar la conexión: `cargo run -- --probar-opgg`.

## Estado v0.10
- [x] Detecta el cliente y se reconecta solo.
- [x] Muestra la fase (sala, buscando, champ select, partida...).
- [x] Muestra tu campeón y tu línea en la selección de campeones.
- [x] Runas recomendadas por Riot (2-3 opciones) con botón Importar.
- [x] Importación automática de runas y hechizos al bloquear (interruptor que se guarda).
- [x] Destello siempre a la derecha (F).
- [x] Interfaz de build con iconos y nombre en inglés al pasar el ratón.
- [x] Builds desde OP.GG: build principal con winrate, botas, inicio, habilidades y situacionales.
- [x] Selector de rol (TOP/JG/MID/ADC/SUP): cambia build y runas.
- [x] Precarga de todos los campeones en segundo plano (tus 10 más jugados primero) y sesión con OP.GG abierta desde el arranque.
- [x] Caché instantánea: enseña la build guardada al momento y la actualiza por detrás.
- [x] Interfaz con arte del campeón, carga animada y logo (monograma K).
- [x] Instalador con acceso directo e icono propio.
- [x] Dos opciones de runas: todas las ligas y Challenger (si es distinta).
- [x] Runas de OP.GG con imágenes (y su winrate). Si no hay hueco, se modifica la página actual.
- [ ] Ayuda de draft: mejores picks contra la composición (solo en champ select).

## Aviso legal
Kite isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot Games or anyone officially involved in producing or managing Riot Games properties. Riot Games, and all associated properties are trademarks or registered trademarks of Riot Games, Inc.

Kite solo usa las APIs oficiales del cliente (LCU), Data Dragon y el servidor MCP público de OP.GG. No lee memoria del juego ni modifica archivos del juego.
