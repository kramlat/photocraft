fn main() {
    for id in std::env::args().skip(1) {
        for c in photocraft_engine::command_specs().iter().filter(|c| c.id == id) {
            println!("{}: {}", c.id, c.params);
        }
    }
}
