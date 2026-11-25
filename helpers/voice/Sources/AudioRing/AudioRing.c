#include "AudioRing.h"
#include <stdatomic.h>
#include <stdlib.h>

struct AudioRing {
    float *data;
    size_t capacity;
    size_t mask;
    _Atomic size_t head;
    _Atomic size_t tail;
};

AudioRing *audio_ring_create(size_t capacity_power_of_two) {
    if (capacity_power_of_two == 0 || (capacity_power_of_two & (capacity_power_of_two - 1)) != 0) {
        return NULL;
    }
    AudioRing *ring = (AudioRing *)calloc(1, sizeof(AudioRing));
    if (!ring) {
        return NULL;
    }
    ring->data = (float *)calloc(capacity_power_of_two, sizeof(float));
    if (!ring->data) {
        free(ring);
        return NULL;
    }
    ring->capacity = capacity_power_of_two;
    ring->mask = capacity_power_of_two - 1;
    atomic_init(&ring->head, 0);
    atomic_init(&ring->tail, 0);
    return ring;
}

void audio_ring_destroy(AudioRing *ring) {
    if (!ring) {
        return;
    }
    free(ring->data);
    free(ring);
}

void audio_ring_reset(AudioRing *ring) {
    atomic_store_explicit(&ring->head, 0, memory_order_relaxed);
    atomic_store_explicit(&ring->tail, 0, memory_order_relaxed);
}

size_t audio_ring_write_mono(
    AudioRing *ring,
    float *const *channels,
    size_t channel_count,
    size_t frames
) {
    if (channel_count == 0 || frames == 0) {
        return 0;
    }
    size_t tail = atomic_load_explicit(&ring->tail, memory_order_relaxed);
    size_t head = atomic_load_explicit(&ring->head, memory_order_acquire);
    size_t available = ring->capacity - (tail - head);
    size_t count = frames < available ? frames : available;
    if (count == 0) {
        return 0;
    }
    size_t mask = ring->mask;
    float *data = ring->data;
    if (channel_count == 1) {
        const float *src = channels[0];
        for (size_t i = 0; i < count; i++) {
            data[(tail + i) & mask] = src[i];
        }
    } else {
        float scale = 1.0f / (float)channel_count;
        for (size_t i = 0; i < count; i++) {
            float sum = 0.0f;
            for (size_t ch = 0; ch < channel_count; ch++) {
                sum += channels[ch][i];
            }
            data[(tail + i) & mask] = sum * scale;
        }
    }
    atomic_store_explicit(&ring->tail, tail + count, memory_order_release);
    return count;
}

size_t audio_ring_read(AudioRing *ring, float *dest, size_t max_frames) {
    size_t head = atomic_load_explicit(&ring->head, memory_order_relaxed);
    size_t tail = atomic_load_explicit(&ring->tail, memory_order_acquire);
    size_t available = tail - head;
    size_t count = max_frames < available ? max_frames : available;
    if (count == 0) {
        return 0;
    }
    size_t mask = ring->mask;
    const float *data = ring->data;
    for (size_t i = 0; i < count; i++) {
        dest[i] = data[(head + i) & mask];
    }
    atomic_store_explicit(&ring->head, head + count, memory_order_release);
    return count;
}
