use std::sync::{Mutex, Arc};

struct Test { value: i32 }
impl Test {
    fn value(&self) -> i32 { self.value }
    fn value_mut(&mut self) -> &mut i32 { &mut self.value }
}

fn do_something(value: Arc<Mutex<Test>>) {
    match { let x = value.lock().unwrap().value(); x } {
        2 => {
            println!("Value is 2");
            *value.lock().unwrap().value_mut() = 3 as i32;
            println!("Value is now 3");
        },
        _ => println!("Value not 2")
    }
}

fn main() {
    let value = Arc::new(Mutex::new(Test{ value: 2 }));
    do_something(value);
}