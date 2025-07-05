
# phira-mp

## Deployment

```shell
# Install Rust first. See https://www.rust-lang.org/tools/install

cargo build --release -p phira-mp-server
RUST_LOG=info target/release/phira-mp-server
```

# About this fork

This fork is based on commit [ce9f9c0](https://github.com/TeamFlos/phira-mp/tree/ce9f9c08032179eabf4c1976fc52bbda746948e8), which powered the `bzlzhh.top:666` server in the past.  
All the changes can be reviewed on the [comparison site](https://github.com/TeamFlos/phira-mp/compare/main...BZLZHH:phira-mp-modified-edition:main).
> [!NOTE]
>
> **Note:** This is an experimental fork and **not** intended for production use.