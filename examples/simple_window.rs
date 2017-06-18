extern crate byteorder;
extern crate tempfile;
#[macro_use]
extern crate wayland_client;
extern crate wayland_protocols;
extern crate wayland_window;

use byteorder::{WriteBytesExt, NativeEndian};

use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;

use tempfile::tempfile;

use wayland_client::{EventQueueHandle, EnvHandler, Proxy};
use wayland_client::protocol::{wl_shell, wl_surface, wl_shm_pool, wl_buffer, wl_compositor,
                               wl_subcompositor, wl_shm};

use wayland_window::DecoratedSurface;

wayland_env!(WaylandEnv,
    compositor: wl_compositor::WlCompositor,
    subcompositor: wl_subcompositor::WlSubcompositor,
    shm: wl_shm::WlShm
);

struct Window {
    s: wl_surface::WlSurface,
    tmp: File,
    pool: wl_shm_pool::WlShmPool,
    pool_size: usize,
    buf: wl_buffer::WlBuffer,
    newsize: Option<(i32, i32)>
}

impl wayland_window::Handler for Window {
    fn configure(&mut self, _: &mut EventQueueHandle, _conf: wayland_window::Configure, width: i32, height: i32) {
        let w = std::cmp::max(width, 100);
        let h = std::cmp::max(height, 100);
        println!("configure: {:?}", (w, h));
        self.newsize = Some((w, h))
    }
    fn close(&mut self, _: &mut EventQueueHandle) {
        println!("close window");
    }
}

impl Window {
    fn resize(&mut self, width: i32, height: i32) {
        if (width*height*4) as usize > self.pool_size {
            // need to reallocate a bigger buffer
            for _ in 0..((width*height) as usize - self.pool_size / 4) {
                self.tmp.write_u32::<NativeEndian>(0xFF880000).unwrap();
            }
            self.pool.resize(width*height*4);
            self.pool_size = (width*height*4) as usize;
        }
        self.buf.destroy();
        self.buf = self.pool.create_buffer(0, width, height, width*4, wl_shm::Format::Argb8888).expect("Pool should not be dead!");
        self.s.attach(Some(&self.buf), 0, 0);
        self.s.commit();
    }
}

fn main() {
    let (display, mut event_queue) = match wayland_client::default_connect() {
        Ok(ret) => ret,
        Err(e) => panic!("Cannot connect to wayland server: {:?}", e)
    };

    event_queue.add_handler(EnvHandler::<WaylandEnv>::new());
    let registry = display.get_registry();
    event_queue.register::<_, EnvHandler<WaylandEnv>>(&registry, 0);
    event_queue.sync_roundtrip().unwrap();

    // Use `xdg-shell` if its available. Otherwise, fall back to `wl-shell`.
    let (mut xdg_shell, mut wl_shell) = (None, None);
    {
        let state = event_queue.state();
        let env = state.get_handler::<EnvHandler<WaylandEnv>>(0);
        for &(name, ref interface, version) in env.globals() {
            use wayland_protocols::unstable::xdg_shell::client::zxdg_shell_v6::ZxdgShellV6;
            if interface == ZxdgShellV6::interface_name() {
                xdg_shell = Some(registry.bind::<ZxdgShellV6>(version, name));
                break;
            }
        }

        if xdg_shell.is_none() {
            for &(name, ref interface, version) in env.globals() {
                if interface == wl_shell::WlShell::interface_name() {
                    wl_shell = Some(registry.bind::<wl_shell::WlShell>(version, name));
                    break;
                }
            }
        }
    }

    let shell = match (xdg_shell, wl_shell) {
        (Some(shell), _) => {
            wayland_window::Shell::Xdg(shell)
        },
        (_, Some(shell)) => {
            wayland_window::Shell::Wl(shell)
        },
        _ => panic!("No available shell"),
    };

    // create a tempfile to write the contents of the window on
    let mut tmp = tempfile().ok().expect("Unable to create a tempfile.");
    // write the contents to it, lets put everything in dark red
    for _ in 0..16 {
        let _ = tmp.write_u32::<NativeEndian>(0xFF880000);
    }
    let _ = tmp.flush();

    // prepare the decorated surface
    let decorated_surface = {
        // introduce a new scope because .state() borrows the event_queue
        let state = event_queue.state();
        // retrieve the EnvHandler
        let env = state.get_handler::<EnvHandler<WaylandEnv>>(0);
        let surface = env.compositor.create_surface();
        let pool = env.shm.create_pool(tmp.as_raw_fd(), 64);
        let buffer = pool.create_buffer(0, 4, 4, 16, wl_shm::Format::Argb8888).expect("I didn't destroy the pool!");

        // find the seat if any
        let mut seat = None;
        for &(id, ref interface, _) in env.globals() {
            if interface == "wl_seat" {
                seat = Some(registry.bind(1, id));
                break;
            }
        }

        let window = Window {
            s: surface,
            tmp: tmp,
            pool: pool,
            pool_size: 64,
            buf: buffer,
            newsize: Some((256, 128))
        };

        let mut decorated_surface = DecoratedSurface::new(&window.s, 16, 16,
            &env.compositor,
            &env.subcompositor,
            &env.shm,
            &shell,
            seat,
            true
        ).unwrap();

        *(decorated_surface.handler()) = Some(window);
        decorated_surface
    };

    event_queue.add_handler_with_init(decorated_surface);

    loop {
        display.flush().unwrap();
        event_queue.dispatch().unwrap();

        // resize if needed
        let mut state = event_queue.state();
        let mut decorated_surface = state.get_mut_handler::<DecoratedSurface<Window>>(1);
        if let Some((w, h)) = decorated_surface.handler().as_mut().unwrap().newsize.take() {
            decorated_surface.resize(w, h);
            decorated_surface.handler().as_mut().unwrap().resize(w, h);
        }
    }
}
