/*
 * Single translation unit holding the implementations of the header-only
 * libraries. Compiled as C so the libraries are built exactly as upstream
 * intends.
 */

/* Only the low-level device API is used. miniaudio's embedded decoders are
 * stripped: files are decoded by the standalone dr_libs below. */
#define MA_NO_DECODING
#define MA_NO_ENCODING
#define MA_NO_GENERATION
#define MA_NO_RESOURCE_MANAGER
#define MA_NO_NODE_GRAPH
#define MA_NO_ENGINE

#define MINIAUDIO_IMPLEMENTATION
#include <miniaudio.h>

#define DR_FLAC_IMPLEMENTATION
#include <dr_flac.h>

#define DR_WAV_IMPLEMENTATION
#include <dr_wav.h>

/* MP3 decoded to float: the int16 build rounds every frame to 16 bits and
 * clips the decoder's overshoot above full scale. */
#define DR_MP3_FLOAT_OUTPUT
#define DR_MP3_IMPLEMENTATION
#include <dr_mp3.h>
