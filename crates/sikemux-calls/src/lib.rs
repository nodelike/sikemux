//! The core's methods a client calls by name, as types: each method's marker, what it takes
//! and what it answers. A client in Rust calls them typed without linking the core, which
//! keeps only what each method runs.

pub mod drafts;

/// One method of the core.
pub trait Call {
    const METHOD: &'static str;
    type Params;
    type Answer;
}

/// Declares methods as `"method" Marker (Params) -> Answer;`.
#[macro_export]
macro_rules! calls {
    ($($name:literal $marker:ident ($takes:ty) -> $gives:ty;)*) => {
        $(
            pub struct $marker;

            impl $crate::Call for $marker {
                const METHOD: &'static str = $name;
                type Params = $takes;
                type Answer = $gives;
            }
        )*
    };
}
