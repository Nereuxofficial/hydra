alias b:= build
alias c:= clean
alias t:= test

clean:
    @echo "Cleaning"
    @cargo clean

test:
    @echo "Running tests"
    @cargo nextest run

fmt:
    @cargo fmt

lint:
    @cargo clippy

build:
    @echo "Building"
    @cargo build
