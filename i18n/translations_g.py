# Strings added with the native PipeWire output and the linear-phase EQ.
T = {
"Converts PCM tracks to DSD with an 8th-order sigma-delta modulator and sends them through the DSD output above. EQ, convolution, ReplayGain and the digital volume are applied before the conversion. DSD512 runs the channels on separate cores.": (
    "Convierte las pistas PCM a DSD con un modulador sigma-delta de 8.º orden y las envía por la salida DSD de arriba. El EQ, la convolución, ReplayGain y el volumen digital se aplican antes de la conversión. DSD512 reparte los canales en varios núcleos.",
    "Converte as faixas PCM em DSD com um modulador sigma-delta de 8ª ordem e as envia pela saída DSD acima. O EQ, a convolução, o ReplayGain e o volume digital são aplicados antes da conversão. DSD512 distribui os canais em vários núcleos.",
    "Convertit les pistes PCM en DSD avec un modulateur sigma-delta du 8e ordre et les envoie par la sortie DSD ci-dessus. L'égaliseur, la convolution, ReplayGain et le volume numérique s'appliquent avant la conversion. DSD512 répartit les canaux sur plusieurs cœurs.",
    "Wandelt PCM-Titel mit einem Sigma-Delta-Modulator 8. Ordnung in DSD um und gibt sie über den DSD-Ausgang oben aus. EQ, Faltung, ReplayGain und die digitale Lautstärke werden vor der Umwandlung angewendet. DSD512 verteilt die Kanäle auf mehrere Kerne.",
    "Converte i brani PCM in DSD con un modulatore sigma-delta dell'8º ordine e li invia all'uscita DSD qui sopra. EQ, convoluzione, ReplayGain e volume digitale si applicano prima della conversione. DSD512 distribuisce i canali su più core."),
"Pitch / tempo (SoundTouch)": (
    "Tono y tempo (SoundTouch)",
    "Tom e tempo (SoundTouch)",
    "Hauteur et tempo (SoundTouch)",
    "Tonhöhe & Tempo (SoundTouch)",
    "Intonazione e tempo (SoundTouch)"),
" · PipeWire converts it to %1 kHz": (
    " · PipeWire lo convierte a %1 kHz",
    " · o PipeWire converte para %1 kHz",
    " · PipeWire le convertit en %1 kHz",
    " · PipeWire wandelt es in %1 kHz um",
    " · PipeWire lo converte a %1 kHz"),
" · linear phase, %1 taps": (
    " · fase lineal, %1 coeficientes",
    " · fase linear, %1 coeficientes",
    " · phase linéaire, %1 coefficients",
    " · linearphasig, %1 Koeffizienten",
    " · fase lineare, %1 coefficienti"),
" · system volume below 100 %": (
    " · volumen del sistema por debajo del 100 %",
    " · volume do sistema abaixo de 100 %",
    " · volume du système inférieur à 100 %",
    " · Systemlautstärke unter 100 %",
    " · volume di sistema sotto il 100 %"),
"Follow the track's rate": (
    "Seguir la frecuencia de la pista",
    "Seguir a taxa da faixa",
    "Suivre la fréquence de la piste",
    "Abtastrate des Titels folgen",
    "Seguire la frequenza del brano"),
"PipeWire switches to each track's sample rate while Sonkore plays.": (
    "PipeWire cambia a la frecuencia de cada pista mientras Sonkore reproduce.",
    "O PipeWire muda para a taxa de amostragem de cada faixa enquanto o Sonkore toca.",
    "PipeWire passe à la fréquence de chaque piste pendant que Sonkore joue.",
    "PipeWire wechselt auf die Abtastrate jedes Titels, solange Sonkore spielt.",
    "PipeWire passa alla frequenza di ogni brano mentre Sonkore riproduce."),
"The desktop's sound server, spoken to directly — shared with other apps; it switches to each track's sample rate (Follow the track's rate).": (
    "El servidor de sonido del escritorio, hablado directamente — compartido con otras apps; cambia a la frecuencia de cada pista (Seguir la frecuencia de la pista).",
    "O servidor de som da área de trabalho, acessado diretamente — compartilhado com outros apps; muda para a taxa de cada faixa (Seguir a taxa da faixa).",
    "Le serveur son du bureau, utilisé directement — partagé avec les autres applications ; il passe à la fréquence de chaque piste (Suivre la fréquence de la piste).",
    "Der Soundserver des Desktops, direkt angesprochen — mit anderen Apps geteilt; er wechselt auf die Abtastrate jedes Titels (Abtastrate des Titels folgen).",
    "Il server audio del desktop, usato direttamente — condiviso con le altre app; passa alla frequenza di ogni brano (Seguire la frequenza del brano)."),
"The same curve as a linear-phase FIR: every frequency is delayed alike, so transients and the stereo image keep their timing. Changes are heard about half a second later.": (
    "La misma curva como un FIR de fase lineal: todas las frecuencias se retrasan por igual, así los transitorios y la imagen estéreo conservan su temporización. Los cambios se escuchan medio segundo después.",
    "A mesma curva como um FIR de fase linear: todas as frequências são atrasadas igualmente, então os transientes e a imagem estéreo mantêm seu tempo. As mudanças são ouvidas cerca de meio segundo depois.",
    "La même courbe en FIR à phase linéaire : toutes les fréquences sont retardées de la même façon, les transitoires et l'image stéréo gardent leur synchronisation. Les changements s'entendent environ une demi-seconde plus tard.",
    "Dieselbe Kurve als linearphasiges FIR: Alle Frequenzen werden gleich verzögert, sodass Transienten und Stereobild ihr Timing behalten. Änderungen sind etwa eine halbe Sekunde später zu hören.",
    "La stessa curva come FIR a fase lineare: tutte le frequenze sono ritardate allo stesso modo, così i transienti e l'immagine stereo mantengono la loro temporizzazione. Le modifiche si sentono circa mezzo secondo dopo."),
"Without it, PipeWire runs at the desktop's rate (usually 48 kHz) and Sonkore converts every track to it with SoX VHQ. Other apps keep playing either way; PipeWire converts them instead. After a minute paused or stopped, PipeWire gets its own rate back.": (
    "Sin esto, PipeWire funciona a la frecuencia del escritorio (normalmente 48 kHz) y Sonkore convierte cada pista con SoX VHQ. Las demás apps siguen sonando igual; PipeWire las convierte a ellas. Tras un minuto en pausa o detenido, PipeWire recupera su frecuencia.",
    "Sem isso, o PipeWire roda na taxa da área de trabalho (geralmente 48 kHz) e o Sonkore converte cada faixa com SoX VHQ. Os outros apps continuam tocando de qualquer forma; o PipeWire os converte. Após um minuto em pausa ou parado, o PipeWire recupera sua taxa.",
    "Sans cela, PipeWire tourne à la fréquence du bureau (généralement 48 kHz) et Sonkore y convertit chaque piste avec SoX VHQ. Les autres applications continuent de jouer dans tous les cas ; c'est PipeWire qui les convertit. Après une minute en pause ou à l'arrêt, PipeWire retrouve sa fréquence.",
    "Ohne diese Option läuft PipeWire mit der Rate des Desktops (meist 48 kHz) und Sonkore wandelt jeden Titel mit SoX VHQ um. Andere Apps spielen in jedem Fall weiter; PipeWire wandelt stattdessen sie um. Nach einer Minute Pause oder Stopp erhält PipeWire seine Rate zurück.",
    "Senza questa opzione PipeWire funziona alla frequenza del desktop (di solito 48 kHz) e Sonkore converte ogni brano con SoX VHQ. Le altre app continuano a suonare in ogni caso; è PipeWire a convertirle. Dopo un minuto in pausa o fermo, PipeWire riprende la sua frequenza."),
"linear phase (FIR)": (
    "fase lineal (FIR)",
    "fase linear (FIR)",
    "phase linéaire (FIR)",
    "linearphasig (FIR)",
    "fase lineare (FIR)"),
}
