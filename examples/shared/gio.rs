use lazy_static::lazy_static;
use std::sync::Mutex;

pub struct SingleEventLoopInstance {
    instance: glib::MainLoop
}

impl SingleEventLoopInstance {
    fn new() -> Self {
        let context = glib::MainContext::new();
        let instance = glib::MainLoop::new(Some(&context), false);

        let instance2 = instance.clone();
        std::thread::spawn(move || {
            if !instance2.get_context().acquire() {
                panic!("failed to acquire event loop context");
            }
            instance2.run();
        });

        SingleEventLoopInstance {
            instance
        }
    }

    pub fn event_loop(&mut self) -> glib::MainLoop {
        self.instance.clone()
    }
}

lazy_static! {
    pub static ref MAIN_GIO_EVENT_LOOP: Mutex<SingleEventLoopInstance> =
                                            Mutex::new(SingleEventLoopInstance::new());
}