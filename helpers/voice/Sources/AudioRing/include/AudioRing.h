#ifndef SIKEMUX_AUDIO_RING_H
#define SIKEMUX_AUDIO_RING_H

#include <stddef.h>

typedef struct AudioRing AudioRing;

AudioRing * _Nullable audio_ring_create(size_t capacity_power_of_two);
void audio_ring_destroy(AudioRing * _Nullable ring);
void audio_ring_reset(AudioRing * _Nonnull ring);
size_t audio_ring_write_mono(
    AudioRing * _Nonnull ring,
    float * _Nonnull const * _Nonnull channels,
    size_t channel_count,
    size_t frames
);
size_t audio_ring_read(AudioRing * _Nonnull ring, float * _Nonnull dest, size_t max_frames);

#endif
