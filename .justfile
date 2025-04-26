_default:
    @just --list

# Run tests
test:
    cargo test

# Run tests with coverage
coverage:
    cargo tarpaulin

# Run check
check:
    cargo check

alias t := test
alias c := check
