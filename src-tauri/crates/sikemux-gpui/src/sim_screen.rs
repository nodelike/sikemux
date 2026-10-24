//! A simulator's screen as its helper streams it: H.264 frames, each a run of
//! units after start codes, decoded by VideoToolbox into pictures GPUI draws
//! with `surface`, on the GPU and without copying them. The helper always sends
//! the screen upright, so a turned device's pictures are turned here, on the GPU too.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use sikemux_process::sync::Locked;

const SPS: u8 = 7;
const PPS: u8 = 8;
const KEY_FRAME: u8 = 5;
/// GPUI's surfaces take bi-planar 4:2:0, full range, as its Metal renderer reads them.
const PIXEL_FORMAT: i32 = i32::from_be_bytes(*b"420f");

type OsStatus = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct CmSampleTimingInfo {
    duration: CmTime,
    presentation: CmTime,
    decode: CmTime,
}

type Decoded = extern "C" fn(
    refcon: *mut c_void,
    frame_refcon: *mut c_void,
    status: OsStatus,
    flags: u32,
    image: *mut c_void,
    presentation: CmTime,
    duration: CmTime,
);

#[repr(C)]
struct DecompressionCallback {
    decoded: Decoded,
    refcon: *mut c_void,
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMTimeInvalid: CmTime;
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: *const c_void,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        unit_header_length: i32,
        out: *mut *const c_void,
    ) -> OsStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: *const c_void,
        memory: *mut c_void,
        length: usize,
        block_allocator: *const c_void,
        custom_source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut *mut c_void,
    ) -> OsStatus;
    fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        destination: *mut c_void,
        offset: usize,
        length: usize,
    ) -> OsStatus;
    fn CMSampleBufferCreateReady(
        allocator: *const c_void,
        data: *mut c_void,
        format: *const c_void,
        samples: isize,
        timings: isize,
        timing: *const CmSampleTimingInfo,
        sizes: isize,
        size: *const usize,
        out: *mut *mut c_void,
    ) -> OsStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: *const c_void,
        format: *const c_void,
        decoder: *const c_void,
        image_attributes: *const c_void,
        callback: *const DecompressionCallback,
        out: *mut *mut c_void,
    ) -> OsStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample: *mut c_void,
        flags: u32,
        frame_refcon: *mut c_void,
        info: *mut u32,
    ) -> OsStatus;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
    static kVTPixelRotationPropertyKey_Rotation: CFStringRef;
    static kVTRotation_CW90: CFStringRef;
    static kVTRotation_180: CFStringRef;
    static kVTRotation_CCW90: CFStringRef;
    fn VTPixelRotationSessionCreate(allocator: *const c_void, out: *mut *mut c_void) -> OsStatus;
    fn VTPixelRotationSessionRotateImage(
        session: *mut c_void,
        source: *mut c_void,
        destination: *mut c_void,
    ) -> OsStatus;
    fn VTPixelRotationSessionInvalidate(session: *mut c_void);
    fn VTSessionSetProperty(
        session: *mut c_void,
        key: CFStringRef,
        value: *const c_void,
    ) -> OsStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    static kCVPixelBufferMetalCompatibilityKey: CFStringRef;
    static kCVPixelBufferWidthKey: CFStringRef;
    static kCVPixelBufferHeightKey: CFStringRef;
    fn CVPixelBufferGetWidth(buffer: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *const c_void) -> usize;
    fn CVPixelBufferPoolCreate(
        allocator: *const c_void,
        pool_attributes: *const c_void,
        buffer_attributes: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn CVPixelBufferPoolCreatePixelBuffer(
        allocator: *const c_void,
        pool: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(object: *const c_void) -> *const c_void;
    fn CFRelease(object: *const c_void);
}

/// The newest picture the decoder made, which the window draws next.
pub type Picture = Arc<Mutex<Option<Frame>>>;

/// How far clockwise, in degrees, the device is turned, which its pictures turn by.
pub type Turn = Arc<AtomicI32>;

/// One decoded picture, held for the window's thread.
pub struct Frame(NonNull<CVPixelBuffer>);

// SAFETY: a pixel buffer is a reference-counted Core Video object, and its reference
// count and IOSurface are safe to use from any thread; the window only reads it.
unsafe impl Send for Frame {}

impl Frame {
    /// The picture, as GPUI's `surface` draws it.
    pub fn buffer(&self) -> CFRetained<CVPixelBuffer> {
        // SAFETY: `self` holds a reference, so the buffer is alive.
        unsafe { CFRetained::retain(self.0) }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: releases the reference `decoded` took.
        unsafe { CFRelease(self.0.as_ptr().cast_const().cast()) };
    }
}

pub struct Decoder {
    sps: Vec<u8>,
    pps: Vec<u8>,
    session: Option<Session>,
    sink: Arc<Sink>,
}

/// Where the session's callback puts pictures, turning them first when the device is turned.
struct Sink {
    picture: Picture,
    turn: Turn,
    rotator: Mutex<Option<Rotator>>,
}

// SAFETY: the session is used from one thread at a time, as the decoder is moved, never
// shared, and its callback only writes to the `Picture` behind its mutex.
unsafe impl Send for Decoder {}

/// A decompression session and the format it was made for, released together.
struct Session {
    session: *mut c_void,
    format: *const c_void,
    /// The callback's own pointer to where pictures go, freed with the session.
    sink: *const Sink,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: both were created by `Decoder::start` and are released once, here; the
        // session is invalidated first, so its callback never runs after its box goes.
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session.cast_const());
            CFRelease(self.format);
            drop(Arc::from_raw(self.sink));
        }
    }
}

impl Decoder {
    pub fn new(picture: Picture, turn: Turn) -> Self {
        Self {
            sps: Vec::new(),
            pps: Vec::new(),
            session: None,
            sink: Arc::new(Sink {
                picture,
                turn,
                rotator: Mutex::new(None),
            }),
        }
    }

    /// Decodes one frame from the helper. A frame with new parameter sets starts a
    /// new session; frames before the first key frame are skipped.
    pub fn decode(&mut self, frame: &[u8]) -> Result<(), String> {
        let units = split_units(frame);
        let mut changed = false;
        for unit in &units {
            match unit.first().map(|header| header & 0x1f) {
                Some(SPS) if self.sps != *unit => {
                    self.sps = unit.to_vec();
                    changed = true;
                }
                Some(PPS) if self.pps != *unit => {
                    self.pps = unit.to_vec();
                    changed = true;
                }
                _ => {}
            }
        }
        if changed || self.session.is_none() {
            let key = units
                .iter()
                .any(|unit| unit.first().map(|header| header & 0x1f) == Some(KEY_FRAME));
            if !key || self.sps.is_empty() || self.pps.is_empty() {
                return Ok(());
            }
            self.session = None;
            self.session = Some(self.start()?);
        }
        let picture: Vec<u8> = units
            .iter()
            .filter(|unit| !matches!(unit.first().map(|header| header & 0x1f), Some(SPS | PPS)))
            .flat_map(|unit| {
                (unit.len() as u32)
                    .to_be_bytes()
                    .into_iter()
                    .chain(unit.iter().copied())
            })
            .collect();
        if picture.is_empty() {
            return Ok(());
        }
        let session = self.session.as_ref().expect("a session was just made");
        // SAFETY: the block buffer allocates and owns its own copy of `picture`; the
        // sample buffer and block buffer are released below after a synchronous decode.
        unsafe {
            let mut block = ptr::null_mut();
            let status = CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                picture.len(),
                ptr::null(),
                ptr::null(),
                0,
                picture.len(),
                0,
                &mut block,
            );
            if status != 0 || block.is_null() {
                return Err(format!(
                    "the screen's frame could not be wrapped ({status})"
                ));
            }
            let status =
                CMBlockBufferReplaceDataBytes(picture.as_ptr().cast(), block, 0, picture.len());
            if status != 0 {
                CFRelease(block.cast_const());
                return Err(format!("the screen's frame could not be copied ({status})"));
            }
            let timing = CmSampleTimingInfo {
                duration: kCMTimeInvalid,
                presentation: kCMTimeInvalid,
                decode: kCMTimeInvalid,
            };
            let size = picture.len();
            let mut sample = ptr::null_mut();
            let status = CMSampleBufferCreateReady(
                ptr::null(),
                block,
                session.format,
                1,
                1,
                &timing,
                1,
                &size,
                &mut sample,
            );
            CFRelease(block.cast_const());
            if status != 0 || sample.is_null() {
                return Err(format!(
                    "the screen's frame could not be sampled ({status})"
                ));
            }
            let mut info = 0;
            let status = VTDecompressionSessionDecodeFrame(
                session.session,
                sample,
                0,
                ptr::null_mut(),
                &mut info,
            );
            CFRelease(sample.cast_const());
            if status != 0 {
                self.session = None;
                return Err(format!(
                    "the screen's frame could not be decoded ({status})"
                ));
            }
        }
        Ok(())
    }

    fn start(&self) -> Result<Session, String> {
        let pointers = [self.sps.as_ptr(), self.pps.as_ptr()];
        let sizes = [self.sps.len(), self.pps.len()];
        let mut format = ptr::null();
        // SAFETY: two parameter sets with their lengths, read during the call only.
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                ptr::null(),
                2,
                pointers.as_ptr(),
                sizes.as_ptr(),
                4,
                &mut format,
            )
        };
        if status != 0 || format.is_null() {
            return Err(format!("the screen's stream could not be read ({status})"));
        }
        // SAFETY: CoreVideo's keys are immortal constants, borrowed here.
        let (pixel_format, surface, metal) = unsafe {
            (
                CFString::wrap_under_get_rule(kCVPixelBufferPixelFormatTypeKey),
                CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey),
                CFString::wrap_under_get_rule(kCVPixelBufferMetalCompatibilityKey),
            )
        };
        let none: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
        let attributes: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
            (pixel_format, CFNumber::from(PIXEL_FORMAT).as_CFType()),
            (surface, none.as_CFType()),
            (metal, CFBoolean::true_value().as_CFType()),
        ]);
        let sink = Arc::into_raw(Arc::clone(&self.sink));
        let callback = DecompressionCallback {
            decoded,
            refcon: sink.cast_mut().cast(),
        };
        let mut session = ptr::null_mut();
        // SAFETY: `format` and the attributes are live; the session copies the callback
        // record, and its refcon box lives until `Session` drops.
        let status = unsafe {
            VTDecompressionSessionCreate(
                ptr::null(),
                format,
                ptr::null(),
                attributes.as_concrete_TypeRef().cast(),
                &callback,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            // SAFETY: made above and not handed to anything else.
            unsafe {
                CFRelease(format);
                drop(Arc::from_raw(sink));
            }
            return Err(format!(
                "the screen's stream could not be decoded ({status})"
            ));
        }
        Ok(Session {
            session,
            format,
            sink,
        })
    }
}

extern "C" fn decoded(
    refcon: *mut c_void,
    _frame_refcon: *mut c_void,
    status: OsStatus,
    _flags: u32,
    image: *mut c_void,
    _presentation: CmTime,
    _duration: CmTime,
) {
    let Some(image) = NonNull::new(image.cast::<CVPixelBuffer>()).filter(|_| status == 0) else {
        return;
    };
    // SAFETY: `refcon` is the session's own `Sink`, alive while the session is, and
    // VideoToolbox lends a live pixel buffer for the length of the call.
    let sink = unsafe { &*refcon.cast_const().cast::<Sink>() };
    let degrees = sink.turn.load(Ordering::Relaxed).rem_euclid(360);
    let turned = match degrees {
        0 => None,
        _ => {
            let mut rotator = sink.rotator.locked();
            rotator.take_if(|rotator| rotator.degrees != degrees);
            if rotator.is_none() {
                *rotator = Rotator::new(degrees);
            }
            rotator.as_mut().and_then(|rotator| rotator.turn(image))
        }
    };
    let frame = turned.unwrap_or_else(|| {
        // SAFETY: the lent buffer is retained for the frame that keeps it.
        unsafe { CFRetain(image.as_ptr().cast_const().cast()) };
        Frame(image)
    });
    *sink.picture.locked() = Some(frame);
}

/// Turns upright pictures by a quarter or half turn into buffers of a pool kept for their size.
struct Rotator {
    degrees: i32,
    session: *mut c_void,
    /// The pool for the turned size it was made for, made with the first picture.
    pool: Option<(usize, usize, *mut c_void)>,
}

// SAFETY: the session and pool are only used behind the sink's mutex.
unsafe impl Send for Rotator {}

impl Rotator {
    fn new(degrees: i32) -> Option<Rotator> {
        let mut session = ptr::null_mut();
        // SAFETY: the session is made fresh and released once, in `drop`; the rotation
        // constants are VideoToolbox's own immortal strings.
        unsafe {
            if VTPixelRotationSessionCreate(ptr::null(), &mut session) != 0 || session.is_null() {
                return None;
            }
            let rotation = match degrees {
                90 => kVTRotation_CW90,
                180 => kVTRotation_180,
                _ => kVTRotation_CCW90,
            };
            VTSessionSetProperty(
                session,
                kVTPixelRotationPropertyKey_Rotation,
                rotation.cast(),
            );
        }
        Some(Rotator {
            degrees,
            session,
            pool: None,
        })
    }

    fn turn(&mut self, image: NonNull<CVPixelBuffer>) -> Option<Frame> {
        let source = image.as_ptr().cast::<c_void>();
        // SAFETY: `image` is a live pixel buffer for the call.
        let (wide, high) = unsafe {
            (
                CVPixelBufferGetWidth(source),
                CVPixelBufferGetHeight(source),
            )
        };
        let size = match self.degrees {
            180 => (wide, high),
            _ => (high, wide),
        };
        if self.pool.is_none_or(|(w, h, _)| (w, h) != size) {
            self.release_pool();
            self.pool = Some((size.0, size.1, pool(size)?));
        }
        let (_, _, pool) = self.pool?;
        let mut turned = ptr::null_mut();
        // SAFETY: the pool and session are this rotator's own and live; the new buffer is
        // owned by the frame that takes it, or released when the rotation fails.
        unsafe {
            if CVPixelBufferPoolCreatePixelBuffer(ptr::null(), pool, &mut turned) != 0
                || turned.is_null()
            {
                return None;
            }
            if VTPixelRotationSessionRotateImage(self.session, source, turned) != 0 {
                CFRelease(turned.cast_const());
                return None;
            }
        }
        NonNull::new(turned.cast::<CVPixelBuffer>()).map(Frame)
    }

    fn release_pool(&mut self) {
        if let Some((_, _, pool)) = self.pool.take() {
            // SAFETY: the pool was made by `pool` and is released once.
            unsafe { CFRelease(pool.cast_const()) };
        }
    }
}

impl Drop for Rotator {
    fn drop(&mut self) {
        self.release_pool();
        // SAFETY: made by `Rotator::new` and released once, here.
        unsafe {
            VTPixelRotationSessionInvalidate(self.session);
            CFRelease(self.session.cast_const());
        }
    }
}

/// A pool of IOSurface buffers `size` pixels across, in the format GPUI's surfaces draw.
fn pool(size: (usize, usize)) -> Option<*mut c_void> {
    // SAFETY: CoreVideo's keys are immortal constants, borrowed here.
    let (pixel_format, surface, metal, width, height) = unsafe {
        (
            CFString::wrap_under_get_rule(kCVPixelBufferPixelFormatTypeKey),
            CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey),
            CFString::wrap_under_get_rule(kCVPixelBufferMetalCompatibilityKey),
            CFString::wrap_under_get_rule(kCVPixelBufferWidthKey),
            CFString::wrap_under_get_rule(kCVPixelBufferHeightKey),
        )
    };
    let none: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
    let attributes: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
        (pixel_format, CFNumber::from(PIXEL_FORMAT).as_CFType()),
        (surface, none.as_CFType()),
        (metal, CFBoolean::true_value().as_CFType()),
        (width, CFNumber::from(size.0 as i64).as_CFType()),
        (height, CFNumber::from(size.1 as i64).as_CFType()),
    ]);
    let mut pool = ptr::null_mut();
    // SAFETY: the attributes are live for the call, which copies them.
    let status = unsafe {
        CVPixelBufferPoolCreate(
            ptr::null(),
            ptr::null(),
            attributes.as_concrete_TypeRef().cast(),
            &mut pool,
        )
    };
    (status == 0 && !pool.is_null()).then_some(pool)
}

/// The units of one frame, each after a 00 00 01 or 00 00 00 01 start code.
fn split_units(frame: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 2 < frame.len() {
        if frame[index] == 0 && frame[index + 1] == 0 {
            if frame[index + 2] == 1 {
                starts.push((index, 3));
                index += 3;
                continue;
            }
            if index + 3 < frame.len() && frame[index + 2] == 0 && frame[index + 3] == 1 {
                starts.push((index, 4));
                index += 4;
                continue;
            }
        }
        index += 1;
    }
    starts
        .iter()
        .enumerate()
        .map(|(at, &(start, length))| {
            let end = starts.get(at + 1).map_or(frame.len(), |&(next, _)| next);
            &frame[start + length..end]
        })
        .collect()
}
