# Sonkore

Reproductor de música hi-fi en Rust, con interfaz en Slint, para Linux y Windows. El usuario habla en español rioplatense y prueba con su pantalla al 150 % y la escala de interfaz en 110 %.

## Reglas

- No hacer commit salvo que el usuario lo pida. Desde el sandbox no hay credenciales para push.
- Nunca reproducir audio en un dispositivo real: las pruebas usan ALSA nulo (`tests/pipewire/alsa-null.conf`) y un PipeWire propio con un sink nulo.
- Las herramientas que se reusan van en `tools/`, no en el scratchpad: el scratchpad se borra.
- Antes de repetir algo lento, medirlo con `$SECONDS` y buscar un camino más corto.

## Comandos (medidos el 2026-11-22, 12 hilos)

| Para | Comando | Tarda |
|---|---|---|
| Compilar mientras se itera | `cargo build -p sonkore` (dev) | ~2 s si cambia el Rust de la app; ~13 s si cambia un `.slint` o un `.po`; 0,3 s sin cambios |
| Revisar todo antes de entregar | `tools/check.sh` (`--todo`: 6 escalas + interfaz 110 %): build sin warnings (también los de Slint), clippy, tests con los dos corpus, Windows, textos, vistas, escenarios | ~3 min; ver resumen al final |
| Opciones de Ajustes que ningún escenario prueba con `guardado` | `tools/cobertura-ajustes.py [--todas]` (`check.sh` muestra el total; las excepciones, con su motivo, en el script) | <1 s |
| Offsets de las estructuras de FFmpeg por versión (al salir un FFmpeg nuevo) | `tools/ffmpeg-offsets.sh` → `crates/ffmpeg/src/abi.rs` | |
| Capturas del README (`docs/screenshots/*.webp`) | `tools/readme/capturas.sh`: la ventana real a 200 % con la biblioteca inventada de `tools/readme/biblioteca.py` | ~45 s la primera vez |
| Código muerto tapado por `#[allow(dead_code)]` | `cargo clippy --workspace -- -A clippy::all --force-warn dead_code` (lo de `win/` y `file_dialog.rs` es de Windows) | ~40 s |
| Vistas sin pantalla, contra una base | `tools/ui-tour.sh [vista…]`; `--base` acepta los cambios, `--hoja` arma una sola imagen | ~5 s + build |
| Ventana real, clics, teclas, audio, latencia | `tools/ui-live.py [escenario…]` (`tools/escenarios/*.txt`) | ~5 s por escenario, en paralelo |
| Avances de Inter con hinting completo a la medida lógica, para `TextMetrics.width` | `tools/avances-hinteados.py` con freetype-py → `crates/app/src/hinted_advances.rs` (correr de nuevo si cambia Inter) | ~1 s |
| Textos de Rust sin traducción (contexto y texto tienen que coincidir con los `i18n/<idioma>.po`) | `tools/textos-sin-traducir.py [es]` (lo corre `check.sh` en los 5 idiomas) | ~1 s |
| CPU de la app | escenario con `cpu NOMBRE S [MAX]`, con `--app target/diario/sonkore` (el build dev exagera) | |
| AppImage para que pruebe el usuario | `packaging/appimage.sh --rapido` (perfil `diario`) | ~22 s |
| AppImage para publicar | `packaging/appimage.sh` (release, LTO fat) | ~4–5 min |
| Flatpak | `packaging/flatpak/build.sh` → `dist/Sonkore-x86_64.flatpak` (lo corre la CI; acá no hay `flatpak-builder`, VS Code corre en un Flatpak) | |
| Borrar compilaciones viejas de `target/` | `tools/limpiar-target.py` | ~1 min |
| Tests del motor | `SONKORE_TEST_CORPUS=target/test-corpus cargo test --workspace` (corpus: `tests/make_corpus.sh`) | |

- Nunca usar `--release` para iterar o sacar capturas: ~253 s por build.
- Si `du -sh target/` pasa de ~16 GB, correr `tools/limpiar-target.py` (~1 min si está compilado): cargo nunca borra las copias viejas, y antes `target/` llegó a 59 GB. Compila lo que usan las herramientas y borra el resto, sin recompilar nada; queda en ~11 GB. `--probar` solo dice cuánto liberaría; `--release` conserva el build de publicar. Lo que más pesa después son las cachés `incremental` de `sonkore-ui` y `sonkore`. `limpiar-target.py` conserva también lo de clippy (con los mismos argumentos que `check.sh`).
- `target` y `dist` son enlaces a `/Projects/Tools/cache/Axiokyn/Sonkore/` (mismo disco, `/Projects/Tools/sacar-cache.sh`), para que la carpeta del proyecto pese solo lo suyo (~50 MB). Si falta el enlace, cargo crea un `target/` nuevo y compila todo de cero: recrearlo con `ln -s /Projects/Tools/cache/Axiokyn/Sonkore/target target`.

## Cómo mirar la interfaz sin gastar tokens

- Primero los textos: lo que imprimen `ui-tour.sh`, `ui-live.py` y `ui-diff.py` (zonas, «Rust corrido +2,+1 px», latencias, estado).
- Después solo los recortes de `target/ui-tour/cambios/`. Nunca abrir 20 capturas enteras: para un vistazo general, `ui-tour.sh --hoja`.

## ui-live: la ventana real sin pantalla

`tools/wl-headless/headless.c` es un compositor Wayland sin pantalla. Tiene escala fraccional, teclado xkb, popups y varias ventanas, y vuelca capturas de cada superficie. Los escenarios usan coordenadas lógicas de la ventana (1320x840) y hacen esto:

- Esperan estados de la app: el gancho `SONKORE_STATE_FILE` (`crates/app/src/probe.rs`) informa página, reproducción, posición, dispositivo, error, cola y cursor.
- Miden la latencia hasta el siguiente cuadro (o el primero de una ventana nueva).
- Revisan agujeros transparentes y daño mal informado.

Comandos que conviene recordar:
- `ajuste seccion/clave=valor` fija una opción en `sonkore.conf` antes de arrancar. Un valor con comas va entre comillas, como lo escribe la app (`escenarios/sonido-por-salida.txt`).
- `preparar COMANDO` (antes de arrancar), `entorno CLAVE=VALOR` (una variable para la app) y `sh COMANDO` (en ese momento); `{rt}` es la carpeta de la corrida y `{repo}` la del proyecto. Con eso `escenarios/dac-enchufado.txt` simula enchufar un DAC: `SONKORE_DEV_SND` y `SONKORE_ASOUND_ROOT` apuntan a carpetas de la prueba.
- `ui-live` espera a que el compositor abra su socket antes de lanzar la app. Antes esperaba 0,3 s fijos y, con varias sesiones a la vez, Rust a veces fallaba con «Could not find wayland compositor» (winit no reintenta).
- `reiniciar` espera a que la app se cierre (por ejemplo con su botón ×, que guarda todo) y la vuelve a abrir con el mismo perfil. Sirve para probar lo que pasa al arrancar (`escenarios/carpeta-quitar.txt`).
- `se-reinicia`: la app se reinicia sola (Ajustes › Reiniciar ahora). Espera a que se cierre bien y sigue con la que ella lanzó (`escenarios/ajustes-general.txt`).
- `guardado seccion/clave=valor` espera a que `sonkore.conf` tenga ese valor (también `~REGEX`, `!~REGEX`, `!=`). Toda opción de Ajustes se prueba así, con el mouse: que se guarde y que el `estado` muestre que se aplicó.
- `pantalla W H` cambia el tamaño de la pantalla del compositor. Con `ajuste widgets/windows/settings=@Size(920 2700)` una pestaña entera de Ajustes entra sin desplazar, y las coordenadas no dependen de la rueda (que avanza 60 px por paso, pida lo que pida). Así están `escenarios/ajustes-*.txt`.
- Los combos de Ajustes abren hacia abajo si entran: la opción i está en y + 43 + 32·i (y: el centro del combo).
- `centrado NOMBRE Y X0 X1` mide en una captura si lo que hay en el renglón Y, entre X0 y X1, deja el mismo espacio a los dos lados (±1 px de pantalla). Sirve para márgenes desparejos como el resaltado de una fila dentro de un menú.
- `arrastrar X Y` verifica que la ventana pida moverse (`xdg_toplevel.move`).
- Soltar archivos sobre la ventana: `soltar-encima X Y RUTA…` (arrastra encima: aparece el aviso, `estado drop=…`), `soltar` y `soltar-fuera`. El compositor ofrece `wl_data_device_manager` solo en esos escenarios (`DND=1`). winit 0.30 no tiene arrastrar y soltar en Wayland: `crates/app/src/dropped.rs` usa su propio `wl_data_device` (`escenarios/soltar-archivos.txt`).
- `cpu NOMBRE S` mide el CPU de la app.
- `perfil NOMBRE S` usa `perf`, que el sandbox no permite.
- En `estado`, las claves pueden tener guiones (`mini-x`, `mini-search`).
- `sonido [S] COND…` mide lo que la app le manda a la salida en los últimos S s (el grifo `SONKORE_OUTPUT_TAP`, `crates/app/src/tap.rs`): `nivel-`, `pico-`, `frec-`, `desvio-` y `preeco-` (con clics: la energía antes del pico, el pre-ringing del filtro de conversión) con `-i`/`-d`, más `tasa`, `bits`, `dsd`, `dop`, `salida`, `salidas` y `silencio` (al empezar). Sin condiciones imprime todo. Toda opción de sonido se prueba así (`escenarios/sonido-*.txt`): que se guarde y que cambie lo que suena.
- Tonos con contenido conocido: `tools/tonos.py` (en `target/ui-live/tonos`). Con `preparar python3 {repo}/tools/tonos.py --biblioteca {rt}/p` y `ajuste library/rescanOnStartup=true` entran a la biblioteca, y `argumentos ARCHIVO…` los reproduce al arrancar (Ctrl+→ pasa al siguiente).
- `ui-live` usa siempre `tests/asound` (las placas no dependen de la máquina), y en el ALSA de prueba todo `hw:` es el dispositivo nulo: la DX1 se puede elegir y tocar, también en DSD nativo y DoP. El grifo marca el tiempo, porque el nulo no lo hace. La clave lleva coma, así que va entre comillas: `ajuste playback/device="alsa|hw:CARD=DX1,DEV=0"`. Sin comillas se guarda como lista y la app ve «alsa|hw:CARD=DX1».
- `arrastre X0 Y0 X1 Y1` aprieta, mueve y suelta (puntos del EQ, sliders). `requiere ARCHIVO` omite el escenario si falta (el corpus opcional de libhdcd). En `sh`, `{pid}` es el proceso de la app (`/proc/{pid}/fd`). `tools/pw-ajuste.py` lee `clock.force-rate` del PipeWire de prueba, y `tools/ritmo.py` mide cuánto avanza la pista por segundo (tempo).
- Ctrl+, abre Ajustes y Ctrl+E el ecualizador: no dependen de la barra, que cambia con la salida.
- Antes de dar por buena una prueba nueva, hay que hacerla fallar quitando el arreglo un momento. Una prueba que pasa igual no reproduce el error.

Los clics llevan hora real y siempre creciente.

Sacar coordenadas de una `captura` de ui-live, no de ui-tour. Con salida PipeWire aparece la etiqueta «Sistema» junto al volumen y corre los botones de la barra inferior: ecualizador 935, salidas 973, mini 1037, ajustes 1074, todos en y=795.

## Trampas conocidas

- La interfaz de Slint compila en su propio crate (`crates/ui`: ~290.000 líneas generadas a partir de `crates/app/ui/*.slint`), y la app la reexporta (`pub use sonkore_ui::*`). Con todo en un crate, rustc volvía a procesar el código generado en cada cambio (~12 s). Su `build.rs` escribe cada `.po` en `OUT_DIR/translations` solo si cambió: slint-build vigila esa carpeta, y rehacerla en cada corrida hacía que cualquier `cargo build`, aunque no cambiara nada, recompilara la interfaz entera (13 s; 2026-11-24).
- El enlace tarda ~0,5 s (Rust ya enlaza con rust-lld): mold no acelera nada.
- Los diálogos de archivos (Ajustes, Ecualizador, ventana principal) van todos por `file_dialog::ask`: portal en un hilo, respuesta en el de la interfaz. zenity bloqueante congelaba la app entera.
- Dentro de un Flatpak solo se pueden tomar los nombres de D-Bus que lista el manifiesto: la bandeja no puede tener `org.kde.StatusNotifierItem-<pid>-<n>` y se registra con su nombre único (`tray.rs`; el test `starts_in_a_flatpak_without_its_own_name` corre de verdad acá, porque VS Code es un Flatpak). Un nombre nuevo en D-Bus va también en `finish-args`. Los archivos que se instalan van en `packaging/linux/install-files.sh`, que usan la AppImage y el Flatpak.
- La CI solo puede usar acciones de GitHub (`actions/*`) y las de la lista del repo (`softprops/action-gh-release`, las de Qt y MSVC de antes): una acción nueva de terceros hace que el workflow entero no arranque (`startup_failure`, 0 jobs). Rust se instala con `rustup` y cargo-deny desde su release, con el checksum.
- `Mpris` tiene dos implementaciones (`platform/src/mpris.rs` y `win/mpris.rs`): lo que se agregue a una va también en la otra, o falla `tools/check-windows.sh`.
- ReplayGain: el reproductor pide la ganancia de cada pista antes de abrirla (`Player::set_gain_source`) y el motor la aplica en la muestra del empalme (`set_next_track_gain`); antes el primer búfer de cada pista sonaba con la ganancia de la anterior.
- El código no está formateado con rustfmt (`cargo fmt --check` da ~1500 diferencias): no correr `cargo fmt` sin que lo pida el usuario.

- `slint::select_bundled_translation` falla (`NoTranslationsBundled`) si se llama antes de crear el primer componente, y Slint se queda con el idioma del sistema: la interfaz salía mezclada (lo de Rust en el idioma elegido, los `@tr` en el del sistema). Se llama con `i18n::select_slint()` después de `MainWindow::new()` (2026-11-23).
- Con el idioma elegido, Slint escribe los decimales con el separador del idioma ("4,5" en español) y `to-float` rechaza el punto. Nunca armar en Slint textos para máquinas con números fraccionarios: los `commands` de un `Path` llevaban "4,66" y la app entraba en pánico en zeno (vista previa «Línea» del visualizador; ahora usa enteros en centésimas de píxel).
- symphonia 0.5 ya recorta el gapless de MP3: no volver a aplicar `packet.trim_*`.
- MP4 necesita su propio parser de elst (usar la duración de mdhd; elst está en ms).
- En AIFF, n_frames viene mal: leer COMM. En ALAC, la profundidad de bits sale del magic cookie.
- Los flags que escriben los callbacks de PipeWire tienen que ser atómicos. Con LTO fat, un campo común leído dentro de `&mut self` se leía una sola vez y nada sonaba.
- `vendor/` tiene parcheados softbuffer, i-slint-backend-winit, i-slint-renderer-software, i-slint-core y parley (los cambios llevan comentarios "Sonkore:"). Hay que reaplicar los parches al actualizar Slint. También symphonia-format-caf, symphonia-format-isomp4 y symphonia-metadata: reservaban lo que dice el archivo (16 TiB por la tabla `pakt` de un CAF, hasta 4 GiB por una imagen FLAC o un frame ID3v2), y una falla de memoria aborta aunque el pánico se pueda atrapar. Reaplicar al actualizar symphonia.
- El perfil release usa `panic = "unwind"`: un pánico de symphonia con un archivo dañado (los hay: `ebml.rs:343`, el índice de `aac/ics`) se atrapa con `sonkore_core::panic_guard::catch` en `decode::open` y en cada lectura del decodificador (`Guarded`), en `metadata::read_any` y en la medición de volumen, y el archivo se saltea como uno ilegible. Con `abort` cerraba la app entera. Cualquier otro pánico es un bug: el hook de `main.rs` cierra la app (si no, quedaba abierta con un hilo muerto). Leer datos de un archivo nuevo = envolverlo en `panic_guard::catch`. En i-slint-core, `DirtyRegion::MAX_COUNT` pasó de 3 a 8, y el renderizador por software también.
- `SONKORE_FRAME_STATS=1` (en `renderer/sw.rs`) imprime cada 2 s cuántos cuadros se dibujaron, cuánto tardaron, cuántos píxeles repintaron y los rectángulos del último. `ui-live` le pasa a la app las variables `SONKORE_*`; queda en `target/ui-live/<run>/app.log` si el escenario falla, o se puede leer con `--app` y un escenario que termine bien.
- Slint guarda su propio estado de "minimizada", y Wayland nunca le avisa cuando la ventana se restaura. Por eso un segundo `set_minimized(true)` no hacía nada: los botones usan `window_drag::minimize`, que va directo a winit.
- Las zonas táctiles de Slint no toman el foco. El global `Pointer` (`window_drag::follow_window`) avisa de cada clic, y el buscador suelta el teclado si el clic cayó afuera (`escenarios/buscador.txt`).
- En Wayland, `drag_window` se come el release del botón: `window_drag.rs` manda uno sintético.
- El renderer por software de Slint ignora la rotación de imágenes: usar íconos ya rotados.
- En Slint, un elemento con `width`/`height` propios y sin `x`/`y` queda **centrado** en el padre. La zona para arrastrar el mini medía lo mismo que el reproductor: sin la cola coincidía con la ventana, y con la cola quedaba en el medio, tapada por la lista. Por eso no se podía arrastrar (corregido el 2026-11-22 con `y: 0`; lo prueba `escenarios/arrastrar.txt`).
- En un `PopupWindow`, lo que va después de `popup.close()` no se ejecuta: cerrar destruye la fila del menú. Los combos de Ajustes cerraban y después avisaban la opción, así que ninguno guardaba nada (idioma, escala, fuente, página de inicio, todo Audio). Primero se avisa y después se cierra (`Combo` en `settings_controls.slint`; `escenarios/ajustes-general.txt`).
- Un `float` de Slint pasado a un `string` sale con la coma del idioma ("-3,5"): Rust lee esos números con `i18n::parse_number`, que acepta coma o punto. Con `parse::<f64>` el headroom manual se guardaba en 0 (`escenarios/ajustes-audio-opciones.txt`).
- Las fuentes de `fonts/` (salvo Inter) se cortaron de fuentes variables y se llamaban «Manrope ExtraLight», «Outfit Thin»…: Slint no encontraba «Manrope» y dibujaba con Inter. `tools/fuentes-nombres.py` les pone el nombre de familia y el peso (`--revisar` solo verifica); el test `every_bundled_font_has_its_family_name` lo fija.
- Un `Flickable` de Slint se desplaza con el arrastre del mouse si no se le pone `mouse-drag-pan-enabled: false`: se quedaba con el arrastre de los puntos del ecualizador. Las ventanas de Ajustes, Ecualizador y Tono lo tienen apagado (2026-11-23).
- `sound_profiles::restore` avisa a las ventanas en la vuelta siguiente del bucle: el que llama tiene `settings` prestado, y con Ajustes abierto elegir una salida con perfil entraba en pánico (`RefCell already mutably borrowed`, `escenarios/sonido-salidas.txt`).
- Un ajuste que cierra la salida (modo de frecuencia, DSD, seguir la frecuencia, exclusivo) cortaba la música, y los que valen al abrir (búfer, HDCD, filtro, desde memoria) recién se oían en la pista siguiente. `extras.rs` reabre la pista donde estaba (`Player::reopen`).
- Lo que no pasa por la ventana de Ajustes (ecualizador, volumen, tono) se escribía recién al cerrar la app: `Controller::save_settings_soon` lo guarda en un segundo.
- El limitador y el headroom no tocan el camino bit-perfect (es el diseño). Para medirlos hace falta DSP activo: ecualizador, crossfeed o volumen digital.
- Una capa con `FocusScope` propio (menús, popups) tiene que devolver el teclado al cerrarse (`App.focus-request`, `menus.rs::restore_keys`). Si no, mueren los atajos (corregido el 2026-11-22).

## Decisiones de diseño (medidas al píxel)

- **Bordes:** `vendor/i-slint-renderer-software` redondea el ancho del borde a píxeles de pantalla enteros, con mínimo 1 (`device_border`). Slint truncaba: 1 px al 150 % daba 1 px de pantalla en vez de 2, y 0,5 px desaparecía, por ejemplo el borde de la etiqueta FLAC. Hay que reaplicar el parche al actualizar Slint.
- **Anillo con punto** (radio de Ajustes › Audio): el anillo y el margen del punto se miden en `phx` enteros y el punto es hijo del anillo; si no, a 165 % el punto quedaba corrido 1 px (`escenarios/ajustes-audio.txt`).
- **Cifras tabulares** en tiempos, contadores, celdas de la lista, `NumField` y la ganancia del ecualizador. Slint no puede activar opciones OpenType, así que esos textos usan `Theme.tnum-family`: Inter con las cifras tabulares fijadas (`fonts/inter/tnum-*.ttf`, generadas por `tools/tnum-fonts.py` con fontTools). `TextMetrics.tnum-width` mide con ellas.
  - Cada glifo que `tnum` reemplaza toma el dibujo y el avance de su forma tabular, y los caracteres siguen apuntando a los glifos de siempre. Redirigir los caracteres a los glifos tabulares rompía el `calt` de Inter: los dos puntos de "0:03" quedaban bajos, y entre cifras tienen que subir. Comprobado con HarfBuzz: la fuente da lo mismo que Inter con `tnum` (avances, posiciones y forma) en los 3 pesos.
  - En el mini van con `tnum` todo el texto de la lista (también el título y el número), la línea del formato, la posición en la cola, los contadores del menú de carpetas y el "100 %". El reloj es un solo texto en esa fuente; armado cifra por cifra con la fuente común, el "1" quedaba sin base.
- **Anchos según el texto:** `PillButton` y `Pill` miden con `TextMetrics.width`, que da los avances con hinting completo a la medida lógica, no con `preferred-width`. Casi siempre es el avance redondeado, pero unos 90 glifos difieren en un píxel (la «s» en 600 a 10 px: «Sistema» mide 41, y 40 redondeando). Esos van en la tabla `hinted_advances.rs` (`tools/avances-hinteados.py`); el test `text_widths_are_hinted_ones` lo fija. `install_metrics` se aplica en la ventana principal, Ajustes, Ecualizador y Pitch.
- **Barra de reproducción:** `PlayButton`: glifo +6, el triángulo corrido 8 %, acento al pasar el mouse, estilos tile y round.
- **Sliders:** `FlatSlider` mide 22 px y dibuja con `SliderFace`, con todos los estilos; también lo usa el mini. La barra de búsqueda queda deshabilitada sin pista. Un clic se convierte en valor como `(x − 6,5) / (ancho − 13)`, no `x / ancho` (con `x / ancho` el mismo clic quedaba ~60 ms más adelante en una pista de 4 s).
- **Barritas de la carpeta en la barra lateral:** en pausa quedan quietas donde estaban. Las de la lista de pistas sí bajan a puntitos (`time` 0).
- **Barra lateral:** el texto de navegación va en x=39 (el borde derecho incluye su último píxel: 12+17−1+11). Los botones de banda miden `section-height − 6`.
- **Logo:** la línea de base usa ascent y descent enteros, y la marca se ubica con división entera.

- **Texto:** con hinting completo, el centrado usa ascent y descent **redondeados a píxeles lógicos** (Inter 13 px: 13 + 3) y la línea de base se redondea al píxel de pantalla más cercano. `vendor/parley` (`full_hinting_metrics` en `style_metrics.rs`) le da a Slint esas métricas: la línea de base coincide en los 900 casos de prueba, y un texto mide ascent + descent. Por eso `UiText` no necesita `line-height-factor`. El "pelito" que evita el `ceil` de Slint se le resta solo al descent, para que una línea de base en x,5 redondee bien.
- **Centrado de layouts:** entre dos estiramientos, y = ⌈(H−h)/2⌉; centrado sin estiramientos, ⌊(H−h)/2⌋. Slint deja fracciones, así que donde hay estiramientos se calcula `padding-top` (`Row` de Ajustes, nombres de la barra de reproducción). Los íconos del árbol de carpetas se centran en (h−1)/2 entero.
- **Barra lateral:** la sección Biblioteca termina en la última fila (`library-full` = banda + 6 + filas − 2), "Carpetas" lleva 4 px arriba y 6 bajo su banda.

- **Dibujos propios** (`crates/app/src/paint.rs`, con tiny-skia, a la escala de la pantalla):
  - el visualizador (barras con topes de pico, espejo, línea y puntos, con subida rápida y bajada suave);
  - el fondo decorado (aurora, degradado y resplandor; la ventana y los paneles quedan al 55 %);
  - la barra forma de onda (los picos los calcula `waveform.rs` en un hilo).
  Slint los muestra como imágenes; los tamaños se los informa la vista a través del global `Visual`.
- **Barra de reproducción:**
  - `share()` nunca deja la parte derecha más angosta que su contenido; si no, el volumen se salía del borde.
  - Sin nada sonando, cuando falta lugar, la parte derecha cede su espacio vacío (hasta su contenido) antes que la pista; si no, «No se está reproduciendo nada» se cortaba al lado de un hueco de ~110 px (a 1040 px de ancho). Con una pista cargada la derecha lo conserva: si no, el transporte se correría con el formato de cada pista (`from-right`).
  - Para decidir si «Sistema»/«DAC» o el visualizador entran, se cuenta el chip de la salida solo si se muestra (`chip-w`, con `App.output-loaded`). Contarlo siempre escondía «Sistema» a 960 px sin nada sonando, aunque entraba.
  - El formato es "FLAC 16-bit · 44.1 kHz", y el artista y el álbum van separados por "  ·  ".
  - Junto a un espacio fijo, el espaciado del layout se pone una sola vez.
  - Una etiqueta con padding y borde suma 6 px de ancho y 2 de alto.
- **Columnas la primera vez** (pedido del usuario, medido en su captura a 165 %): Nombre de archivo · Formato 113 · Tiempo 86 · ♥. Formato solo crece si una etiqueta no entra. Mientras el nombre no tenga un ancho guardado, el espacio que sobra va al nombre, no a Tiempo (`tracklist.rs`, test `first_launch_columns`).
- **Lista:** en la densidad Lista las filas no dejan el píxel de separación (`Theme.row-pitch`), y la columna del número mide 36 en todas las densidades. Si cambian la densidad o las carátulas, la lista vuelve a distribuir las columnas (`list-look-changed`); si no, la columna de carátulas quedaba y las carátulas se encimaban.

- **Sonido, opciones que antes no hacían nada** (2026-11-23):
  - Las señales de prueba (`Controller::play_test_signal`) guardan la cola y la devuelven al terminar o al detenerlas.
  - «Devolver el DAC en pausa»: después de un minuto con una salida directa abierta sin sonar (`release_when_idle`; `SONKORE_IDLE_RELEASE_MS` lo acorta en las pruebas). `Player::release_output` guarda la posición, y antes Play no volvía a abrir la salida.
  - «Desde memoria»: `decode::Prefetch` lee el archivo en un hilo propio y lo cierra; el decodificador espera solo los bytes que pide.
  - ReplayGain se aplica en el momento a la pista que suena, también a archivos fuera de la biblioteca (lee sus etiquetas), y con el modo prendido la biblioteca mide en segundo plano las pistas sin etiquetas.
  - «Liberar DAC» estaba siempre deshabilitado (`output_reserved` no se completaba), y con PipeWire aparecían las filas del DAC y «exclusivo».
- **Salidas:** `audio::output_list` arma la lista: primero "Predeterminado del sistema", y las salidas de PipeWire de la tarjeta reservada se recuerdan, porque PipeWire las borra mientras Sonkore la tiene tomada. Se usa en el menú y en Ajustes.
- **Contorno de la ventana:** ninguno mientras está maximizada.
- **Barras de desplazamiento:** todas son la `OverlayScrollBar` de `controls.slint`, oculta con la lista quieta, visible mientras se desplaza y 0,9 s después, o con el puntero encima. El `ListView`/`ScrollView` de Slint dejaba su barra siempre a la vista (`escenarios/barras.txt`).
- **Barra lateral en una ventana baja:** cuando no entra todo, primero Carpetas baja a su mínimo, después Biblioteca y Listas ceden lo mismo cada una (el píxel impar lo cede Biblioteca), cada una hasta su banda más 62 px. Con las secciones fijas, la barra lateral imponía su mínimo a la ventana y empujaba la barra de reproducción fuera por debajo de ~760 px de alto.
- **Barra lateral en una ventana angosta:** baja hasta 200 px cuando la página no entra. Mínimos de cada página: Álbumes 567 (481 compacta), Artistas/Géneros 532 (516), Sonando ahora 471 (ninguno en compacta); Pistas y las demás nunca la hacen ceder (`main.slint`, `page-least`). No se midió con un álbum abierto.

- **Clic derecho en una carpeta del árbol** (pedido del usuario): abre el menú de sus pistas (Reproducir, Agregar a la cola, Agregar a la lista, Abrir la carpeta contenedora) y "Quitar carpeta de la biblioteca", que pide confirmación (`folder_menu`). Se ofrece también en las subcarpetas, que quedan omitidas, como el tacho del mini (`escenarios/carpeta-arbol.txt`).
- **Quitar u omitir una carpeta:** sus pistas salen de la biblioteca y `library.json` se guarda en ese momento (`forget_under`), sin esperar al re-escaneo. Si no, al cerrar la app antes de que terminara el escaneo la carpeta volvía a aparecer (prueba `removed_folders_stay_gone_after_quitting_mid_scan`). Desde el mini: `escenarios/carpeta-quitar.txt` quita la carpeta que suena, cierra, vuelve a abrir con re-escaneo y vigilancia, y la carpeta sigue afuera.
- **Traducciones:** están en `i18n/<idioma>.po`; `i18n/update.py` agrega los textos nuevos y los llena desde `i18n/translations_*.py`. Los marcadores van numerados (`{0}`), en `@tr` y en `tr`/`trf` de Rust: un `{}` no encuentra su traducción.
- **Menús del mini:** el contenido queda recortado dentro de los dos anillos del borde (`MenuCard`), y una lista desplazada no pasa por encima del borde. Los seis menús los mide `escenarios/menus-mini.txt` y `menu-carpetas.txt`: 4 px de fondo a cada lado de una fila resaltada. Dentro de `MenuCard`, `parent` es la tarjeta y no el rectángulo que recorta, así que las listas van en `x: -2px` con el ancho de `parent`, sin sumar nada. Con `+4px`, cada fila salía 4 px más ancha y su resaltado tocaba el borde derecho.
- **Lista del mini:** si cambian las carátulas o la densidad con el mini abierto, sus columnas se vuelven a distribuir (`list-look-changed`).
- **Barra de reproducción angosta** (pedido del usuario): cuando no entra, cede en este orden: la pastilla de la ruta queda solo con el ícono, después se oculta "Sistema/DAC" y por último el visualizador. La etiqueta de formato se acorta con "…" (`Badge.limit`).
- **Niveles de subcarpetas:** una sola fila con "Subcarpetas", el selector − N + (de 1 hasta la profundidad de la biblioteca) y un interruptor, y el menú queda abierto mientras se usa. En los menús es una fila `Entry::levels`, cuyos botones llaman a `Menus.kept`; en el mini es `LevelsRow`, y su menú se cierra solo con clics afuera.
- Ctrl+M alterna entre los dos modos. Al cambiar de modo el teclado vuelve a los atajos, y al entrar al mini la lista muestra la pista que suena (`escenarios/modos.txt`).
- La lista del mini, al crearse de nuevo, vuelve adonde estaba (`Mini.scrolled-to`) y se lo avisa a Rust; si no, la lista nueva arrancaba arriba y se veía vacía hasta desplazarla. Con 12 pistas no se reproduce: la ventana completa las muestra todas y la posición vuelve a 0.
- El buscador del mini también suelta el teclado con un clic afuera (`escenarios/mini-buscador.txt`).
- Las barritas de la carpeta que suena también se mueven en el mini.
- Un clic en la carpeta de la que suena la música selecciona y centra la pista (`reveal_playing`, `escenarios/carpeta.txt`).
- El ícono de play de las vistas previas de los controles queda centrado.
- La forma de onda también aparece en la barra del mini.
- El visualizador va sobre la línea de progreso.

**Cada cuadro de Slint cuesta alrededor de 1 ms** con la ventana entera, aunque cambie un píxel. Lo que se mueve tiene que compartir cuadro: el visualizador, las barritas y la posición avanzan juntos en un solo temporizador de 33 ms (`watch_visual` y `tick_visual`). Las propiedades se asignan solo si cambiaron, y nunca se crea un modelo nuevo por cuadro: las barritas leen `App.bars-time`. Con solo 3 zonas sucias, Slint unía las barritas, el visualizador y la barra de progreso en rectángulos de un cuarto de la ventana (530.000 px por cuadro); con 8 repinta 139.000. Medido con el perfil `diario` y música sonando: 4,7 %; en pausa, 0 %.

Pendiente de la interfaz:

- Ya no quedan opciones de sonido sin efecto: PCM → DSD funciona (nativo y DoP, `escenarios/sonido-pcm-dsd.txt`) y las fases «intermedia» y «mínima» del filtro de conversión son un FIR polifásico propio (`crates/dsp/src/polyphase.rs`: fase mínima por cepstro, intermedia como la de soxr; `docs/engine.md`). `escenarios/sonido-filtro.txt` las distingue con clics (`preeco`): lineal −23,7 dB, intermedia −51,5, mínima nada.

- Slint no ajusta la forma de cada glifo como el autohinter de FreeType (por ejemplo la base del "2" o el ojo de la "t"): quedan diferencias de ±0,5 px en el dibujo de las letras, con las posiciones bien.
- Con una fuente que no es Inter, los anchos medidos (`TextMetrics`, `metrics.rs`) usan sus avances redondeados, sin la tabla de hinting de Inter: puede faltar o sobrar un píxel en pastillas y columnas.

Para ver lo que hace Slint con un texto: `cargo run -p sonkore --example textprobe`.

En el compositor de prueba, winit necesita `wl_surface.enter` para `current_monitor()`; sin eso, las ventanas no se limitaban al alto de la pantalla.
