use cherry_pit_core::EventScheduler;

type ErasedScheduler = Box<dyn EventScheduler<Error = std::convert::Infallible>>;

fn main() {
    let _erased: Option<ErasedScheduler> = None;
}
