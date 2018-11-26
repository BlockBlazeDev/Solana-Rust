[![Solana crate](https://img.shields.io/crates/v/solana.svg)](https://crates.io/crates/solana)
[![Solana documentation](https://docs.rs/solana/badge.svg)](https://docs.rs/solana)
[![Build status](https://badge.buildkite.com/d4c4d7da9154e3a8fb7199325f430ccdb05be5fc1e92777e51.svg?branch=master)](https://solana-ci-gate.herokuapp.com/buildkite_public_log?https://buildkite.com/solana-labs/solana/builds/latest/master)
[![codecov](https://codecov.io/gh/solana-labs/solana/branch/master/graph/badge.svg)](https://codecov.io/gh/solana-labs/solana)

Blockchain Rebuilt for Scale
===

Solana&trade; is a new blockchain architecture built from the ground up for scale. The architecture supports
up to 710 thousand transactions per second on a gigabit network.

Disclaimer
===

All claims, content, designs, algorithms, estimates, roadmaps, specifications, and performance measurements described in this project are done with the author's best effort.  It is up to the reader to check and validate their accuracy and truthfulness.  Furthermore nothing in this project constitutes a solicitation for investment.

Introduction
===

It's possible for a centralized database to process 710,000 transactions per second on a standard gigabit network if the transactions are, on average, no more than 176 bytes. A centralized database can also replicate itself and maintain high availability without significantly compromising that transaction rate using the distributed system technique known as Optimistic Concurrency Control [\[H.T.Kung, J.T.Robinson (1981)\]](http://citeseerx.ist.psu.edu/viewdoc/summary?doi=10.1.1.65.4735). At Solana, we're demonstrating that these same theoretical limits apply just as well to blockchain on an adversarial network. The key ingredient? Finding a way to share time when nodes can't trust one-another. Once nodes can trust time, suddenly ~40 years of distributed systems research becomes applicable to blockchain!

> Perhaps the most striking difference between algorithms obtained by our method and ones based upon timeout is that using timeout produces a traditional distributed algorithm in which the processes operate asynchronously, while our method produces a globally synchronous one in which every process does the same thing at (approximately) the same time. Our method seems to contradict the whole purpose of distributed processing, which is to permit different processes to operate independently and perform different functions. However, if a distributed system is really a single system, then the processes must be synchronized in some way. Conceptually, the easiest way to synchronize processes is to get them all to do the same thing at the same time. Therefore, our method is used to implement a kernel that performs the necessary synchronization--for example, making sure that two different processes do not try to modify a file at the same time. Processes might spend only a small fraction of their time executing the synchronizing kernel; the rest of the time, they can operate independently--e.g., accessing different files. This is an approach we have advocated even when fault-tolerance is not required. The method's basic simplicity makes it easier to understand the precise properties of a system, which is crucial if one is to know just how fault-tolerant the system is. [\[L.Lamport (1984)\]](http://citeseerx.ist.psu.edu/viewdoc/summary?doi=10.1.1.71.1078)

Furthermore, and much to our surprise, it can be implemented using a mechanism that has existed in Bitcoin since day one. The Bitcoin feature is called nLocktime and it can be used to postdate transactions using block height instead of a timestamp. As a Bitcoin client, you'd use block height instead of a timestamp if you don't trust the network. Block height turns out to be an instance of what's being called a Verifiable Delay Function in cryptography circles. It's a cryptographically secure way to say time has passed. In Solana, we use a far more granular verifiable delay function, a SHA 256 hash chain, to checkpoint the ledger and coordinate consensus. With it, we implement Optimistic Concurrency Control and are now well en route towards that theoretical limit of 710,000 transactions per second.

Architecture
===

Before you jump into the code, review the online book [Solana: Blockchain Rebuilt for Scale](https://solana-labs.github.io/solana/).

Developing
===

Building
---

Install rustc, cargo and rustfmt:

```bash
$ curl https://sh.rustup.rs -sSf | sh
$ source $HOME/.cargo/env
$ rustup component add rustfmt-preview
```

If your rustc version is lower than 1.26.1, please update it:

```bash
$ rustup update
```

On Linux systems you may need to install libssl-dev, pkg-config, zlib1g-dev, etc.  On Ubuntu:

```bash
$ sudo apt-get install libssl-dev pkg-config zlib1g-dev llvm
```

Download the source code:

```bash
$ git clone https://github.com/solana-labs/solana.git
$ cd solana
```

Building the Solana book
---

Install mdbook:

```bash
cargo install mdbook
```

Run any Rust tests in the markdown:

```bash
make -C book test
```

Render markdown as HTML:

```bash
make -C book build
```

Render and view the book:

```bash
make -C book open
```


Testing
---

Run the test suite:

```bash
$ cargo test
```

To emulate all the tests that will run on a Pull Request, run:

```bash
$ ./ci/run-local.sh
```

Fullnode Debugging
---

There are some useful debug messages in the code, you can enable them on a per-module and per-level
basis.  Before running a leader or validator set the normal RUST\_LOG environment variable.

For example

* To enable `info` everywhere and `debug` only in the solana::banking_stage module:

  ```bash
  $ export RUST_LOG=info,solana::banking_stage=debug
  ```

* To enable BPF program logging:

  ```bash
  $ export RUST_LOG=solana_bpf_loader
  ```

Generally we are using `debug` for infrequent debug messages, `trace` for potentially frequent
messages and `info` for performance-related logging.

You can also attach to a running process with GDB.  The leader's process is named
_solana-fullnode_:

```bash
$ sudo gdb
attach <PID>
set logging on
thread apply all bt
```

This will dump all the threads stack traces into gdb.txt


Testnet Debugging
---

We maintain several testnets:

* `testnet` - public stable testnet accessible via testnet.solana.com, with an https proxy for web apps at api.testnet.solana.com. Runs 24/7
* `testnet-beta` - public beta channel testnet accessible via beta.testnet.solana.com. Runs 24/7
* `testnet-edge` - public edge channel testnet accessible via edge.testnet.solana.com. Runs 24/7
* `testnet-perf` - permissioned stable testnet running a 24/7 soak test
* `testnet-beta-perf` - permissioned beta channel testnet running a multi-hour soak test weekday mornings
* `testnet-edge-perf` - permissioned edge channel testnet running a multi-hour soak test weekday mornings

## Deploy process

They are deployed with the `ci/testnet-manager.sh` script through a list of [scheduled
buildkite jobs](https://buildkite.com/solana-labs/testnet-management/settings/schedules).
Each testnet can be manually manipulated from buildkite as well.  The `-perf`
testnets use a release tarball while the non`-perf` builds use the snap build
(we've observed that the snap build runs slower than a tarball but this has yet
to be root caused).

## Where are the testnet logs?
**TODO: This section is out of date and needs updating**

Attach to the testnet first by running one of:
```bash
$ net/gce.sh config testnet-solana-com
$ net/gce.sh config master-testnet-solana-com
$ net/gce.sh config perf-testnet-solana-com
```

Then run:
```bash
$ net/ssh.sh
```
for log location details

## How do I reset the testnet?
Manually trigger the [testnet-management](https://buildkite.com/solana-labs/testnet-management) pipeline
and when prompted select the desired testnet

## How can I scale the tx generation rate?

Increase the TX rate by increasing the number of cores on the client machine which is running
`bench-tps` or run multiple clients. Decrease by lowering cores or using the rayon env
variable `RAYON_NUM_THREADS=<xx>`

## How can I test a change on the testnet?

Currently, a merged PR is the only way to test a change on the testnet.  But you
can run your own testnet using the scripts in the `net/` directory.

## Adjusting the number of clients or validators on the testnet
Edit `ci/testnet-manager.sh`


Benchmarking
---

First install the nightly build of rustc. `cargo bench` requires unstable features:

```bash
$ rustup install nightly
```

Run the benchmarks:

```bash
$ cargo +nightly bench --features="unstable"
```

Release Process
---
The release process for this project is described [here](rfcs/0005-branches-tags-and-channels.md).


Code coverage
---

To generate code coverage statistics, install cargo-cov. Note: the tool currently only works
in Rust nightly.

```bash
$ cargo +nightly install cargo-cov
```

Run cargo-cov and generate a report:

```bash
$ cargo +nightly cov test
$ cargo +nightly cov report --open
```

The coverage report will be written to `./target/cov/report/index.html`


Why coverage? While most see coverage as a code quality metric, we see it primarily as a developer
productivity metric. When a developer makes a change to the codebase, presumably it's a *solution* to
some problem.  Our unit-test suite is how we encode the set of *problems* the codebase solves. Running
the test suite should indicate that your change didn't *infringe* on anyone else's solutions. Adding a
test *protects* your solution from future changes. Say you don't understand why a line of code exists,
try deleting it and running the unit-tests. The nearest test failure should tell you what problem
was solved by that code. If no test fails, go ahead and submit a Pull Request that asks, "what
problem is solved by this code?" On the other hand, if a test does fail and you can think of a
better way to solve the same problem, a Pull Request with your solution would most certainly be
welcome! Likewise, if rewriting a test can better communicate what code it's protecting, please
send us that patch!


