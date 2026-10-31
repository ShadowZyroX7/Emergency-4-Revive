//! OpenGL entry points used by the post-processing pipeline.
//!
//! GL 1.1 functions come straight from opengl32.dll; GL 2.0 shader and
//! framebuffer-object functions via wglGetProcAddress (needs a current
//! context, so this is loaded lazily on the render thread).

#![allow(non_snake_case)]

use em4r_core::util::loaded_proc;

pub const GL_TEXTURE_2D: u32 = 0x0DE1;
pub const GL_RGBA: u32 = 0x1908;
pub const GL_RGBA8: u32 = 0x8058;
pub const GL_UNSIGNED_BYTE: u32 = 0x1401;
pub const GL_FLOAT: u32 = 0x1406;
pub const GL_DEPTH_COMPONENT: u32 = 0x1902;
pub const GL_DEPTH_COMPONENT24: u32 = 0x81A6;
pub const GL_DEPTH24_STENCIL8: u32 = 0x88F0;
pub const GL_DEPTH_STENCIL: u32 = 0x84F9;
pub const GL_UNSIGNED_INT_24_8: u32 = 0x84FA;
pub const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
pub const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
pub const GL_TEXTURE_WRAP_S: u32 = 0x2802;
pub const GL_TEXTURE_WRAP_T: u32 = 0x2803;
pub const GL_LINEAR: i32 = 0x2601;
pub const GL_NEAREST: i32 = 0x2600;
pub const GL_CLAMP_TO_EDGE: i32 = 0x812F;
pub const GL_TEXTURE_COMPARE_MODE: u32 = 0x884C;
pub const GL_NONE: i32 = 0;
pub const GL_FRAMEBUFFER: u32 = 0x8D40;
pub const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
pub const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
pub const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
pub const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
pub const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
pub const GL_DEPTH_STENCIL_ATTACHMENT: u32 = 0x821A;
pub const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
pub const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
pub const GL_DEPTH_BUFFER_BIT: u32 = 0x0100;
pub const GL_STENCIL_BUFFER_BIT: u32 = 0x0400;
pub const GL_ALL_ATTRIB_BITS: u32 = 0x000F_FFFF;
pub const GL_VIEWPORT: u32 = 0x0BA2;
pub const GL_CURRENT_PROGRAM: u32 = 0x8B8D;
pub const GL_ACTIVE_TEXTURE: u32 = 0x84E0;
pub const GL_TEXTURE0: u32 = 0x84C0;
pub const GL_VERTEX_SHADER: u32 = 0x8B31;
pub const GL_FRAGMENT_SHADER: u32 = 0x8B30;
pub const GL_COMPILE_STATUS: u32 = 0x8B81;
pub const GL_LINK_STATUS: u32 = 0x8B82;
pub const GL_QUADS: u32 = 0x0007;
pub const GL_DEPTH_TEST: u32 = 0x0B71;
pub const GL_BLEND: u32 = 0x0BE2;
pub const GL_ALPHA_TEST: u32 = 0x0BC0;
pub const GL_CULL_FACE: u32 = 0x0B44;
pub const GL_SCISSOR_TEST: u32 = 0x0C11;
pub const GL_STENCIL_TEST: u32 = 0x0B90;
pub const GL_FOG: u32 = 0x0B60;
pub const GL_LIGHTING: u32 = 0x0B50;
pub const GL_BACK: u32 = 0x0405;
pub const GL_NO_ERROR: u32 = 0;

macro_rules! gl_fns {
    ($( $name:ident : fn($($arg:ty),*) $(-> $ret:ty)? ; )*) => {
        /// Function table; all pointers are non-null after a successful load.
        pub struct Gl {
            $( pub $name: unsafe extern "system" fn($($arg),*) $(-> $ret)?, )*
        }

        impl Gl {
            /// Resolve everything. `None` (and the name) if anything is missing.
            pub unsafe fn load() -> Result<Gl, &'static str> {
                type GetProc = unsafe extern "system" fn(*const u8) -> usize;
                let gpa: GetProc = match loaded_proc("opengl32.dll", "wglGetProcAddress") {
                    Some(p) => core::mem::transmute(p),
                    None => return Err("wglGetProcAddress"),
                };
                Ok(Gl {
                    $( $name: {
                        let n = stringify!($name);
                        let cname = concat!(stringify!($name), "\0");
                        let mut p = loaded_proc("opengl32.dll", n).unwrap_or(0);
                        if p == 0 {
                            p = gpa(cname.as_ptr());
                        }
                        // wglGetProcAddress may return small sentinel values on failure.
                        if p <= 3 || p == usize::MAX {
                            return Err(n);
                        }
                        core::mem::transmute(p)
                    }, )*
                })
            }
        }
    };
}

gl_fns! {
    // GL 1.1
    glGetError: fn() -> u32;
    glGetIntegerv: fn(u32, *mut i32);
    glEnable: fn(u32);
    glDisable: fn(u32);
    glPushAttrib: fn(u32);
    glPopAttrib: fn();
    glViewport: fn(i32, i32, i32, i32);
    glGenTextures: fn(i32, *mut u32);
    glDeleteTextures: fn(i32, *const u32);
    glBindTexture: fn(u32, u32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const u8);
    glTexParameteri: fn(u32, u32, i32);
    glDepthMask: fn(u8);
    glColorMask: fn(u8, u8, u8, u8);
    glBegin: fn(u32);
    glEnd: fn();
    glVertex2f: fn(f32, f32);
    glDrawBuffer: fn(u32);
    glReadBuffer: fn(u32);
    glReadPixels: fn(i32, i32, i32, i32, u32, u32, *mut u8);
    // GL 1.3
    glActiveTexture: fn(u32);
    // GL 2.0 shaders
    glCreateShader: fn(u32) -> u32;
    glShaderSource: fn(u32, i32, *const *const u8, *const i32);
    glCompileShader: fn(u32);
    glGetShaderiv: fn(u32, u32, *mut i32);
    glGetShaderInfoLog: fn(u32, i32, *mut i32, *mut u8);
    glCreateProgram: fn() -> u32;
    glAttachShader: fn(u32, u32);
    glLinkProgram: fn(u32);
    glGetProgramiv: fn(u32, u32, *mut i32);
    glGetProgramInfoLog: fn(u32, i32, *mut i32, *mut u8);
    glUseProgram: fn(u32);
    glGetUniformLocation: fn(u32, *const u8) -> i32;
    glUniform1i: fn(i32, i32);
    glUniform1f: fn(i32, f32);
    glUniform2f: fn(i32, f32, f32);
    // GL 3.0 / ARB_framebuffer_object
    glGenFramebuffers: fn(i32, *mut u32);
    glBindFramebuffer: fn(u32, u32);
    glFramebufferTexture2D: fn(u32, u32, u32, u32, i32);
    glCheckFramebufferStatus: fn(u32) -> u32;
    glBlitFramebuffer: fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32);
}
