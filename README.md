[![Solana crate](https://img.shields.io/crates/v/solana.svg)](https://crates.io/crates/solana)
[![Solana documentation](https://docs.rs/solana/badge.svg)](https://docs.rs/solana)
[![Build status](https://badge.buildkite.com/d4c4d7da9154e3a8fb7199325f430ccdb05be5fc1e92777e51.svg?branch=master)](https://solana-ci-gate.herokuapp.com/buildkite_public_log?https://buildkite.com/solana-labs/solana/builds/latest/master)
[![codecov](https://codecov.io/gh/solana-labs/solana/branch/master/graph/badge.svg)](https://codecov.io/gh/solana-labs/solana)

Blockchain, Rebuilt for Scale
===

Solana&trade; is a new blockchain architecture built from the ground up for scale. The architecture supports
up to 710 thousand transactions per second on a gigabit network.

Disclaimer
===

All claims, content, designs, algorithms, estimates, roadmaps, specifications, and performance measurements described in this project are done with the author's best effort.  It is up to the reader to check and validate their accuracy and truthfulness.  Furthermore nothing in this project constitutes a solicitation for investment.

Introduction
===

It's possible for a centralized database to process 710,000 transactions per second on a standard gigabit network if the transactions are, on average, no more than 176 bytes. A centralized database can also replicate itself and maintain high availability without significantly compromising that transaction rate using the distributed system technique known as Optimistic Concurrency Control [H.T.Kung, J.T.Robinson (1981)]. At Solana, we're demonstrating that these same theoretical limits apply just as well to blockchain on an adversarial network. The key ingredient? Finding a way to share time when nodes can't trust one-another. Once nodes can trust time, suddenly ~40 years of distributed systems research becomes applicable to blockchain! Furthermore, and much to our surprise, it can implemented using a mechanism that has existed in Bitcoin since day one. The Bitcoin feature is called nLocktime and it can be used to postdate transactions using block height instead of a timestamp. As a Bitcoin client, you'd use block height instead of a timestamp if you don't trust the network. Block height turns out to be an instance of what's being called a Verifiable Delay Function in cryptography circles. It's a cryptographically secure way to say time has passed. In Solana, we use a far more granular verifiable delay function, a SHA 256 hash chain, to checkpoint the ledger and coordinate consensus. With it, we implement Optimistic Concurrency Control and are now well in route towards that theoretical limit of 710,000 transactions per second.


Testnet Demos
===

The Solana repo contains all the scripts you might need to spin up your own
local testnet. Depending on what you're looking to achieve, you may want to
run a different variation, as the full-fledged, performance-enhanced
multinode testnet is considerably more complex to set up than a Rust-only,
singlenode testnode.  If you are looking to develop high-level features, such
as experimenting with smart contracts, save yourself some setup headaches and
stick to the Rust-only singlenode demo.  If you're doing performance optimization
of the transaction pipeline, consider the enhanced singlenode demo. If you're
doing consensus work, you'll need at least a Rust-only multinode demo. If you want
to reproduce our TPS metrics, run the enhanced multinode demo.

For all four variations, you'd need the latest Rust toolchain and the Solana
source code:

First, install Rust's package manager Cargo.

```bash
$ curl https://sh.rustup.rs -sSf | sh
$ source $HOME/.cargo/env
```

Now checkout the code from github:

```bash
$ git clone https://github.com/solana-labs/solana.git
$ cd solana
```

The demo code is sometimes broken between releases as we add new low-level
features, so if this is your first time running the demo, you'll improve
your odds of success if you check out the
[latest release](https://github.com/solana-labs/solana/releases)
before proceeding:

```bash
$ git checkout v0.7.0-beta
```

Configuration Setup
---

The network is initialized with a genesis ledger and leader/validator configuration files.
These files can be generated by running the following script.

```bash
$ ./multinode-demo/setup.sh
```

Singlenode Testnet
---

Before you start a fullnode, make sure you know the IP address of the machine you
want to be the leader for the demo, and make sure that udp ports 8000-10000 are
open on all the machines you want to test with.

Now start the server:

```bash
$ ./multinode-demo/leader.sh
```

Wait a few seconds for the server to initialize. It will print "Ready." when it's ready to
receive transactions.

Drone
---

In order for the below test client and validators to work, we'll also need to
spin up a drone to give out some test tokens.  The drone delivers Milton
Friedman-style "air drops" (free tokens to requesting clients) to be used in
test transactions.

Start the drone on the leader node with:

```bash
$ ./multinode-demo/drone.sh
```


Multinode Testnet
---

To run a multinode testnet, after starting a leader node, spin up some validator nodes:

```bash
$ ./multinode-demo/validator.sh ubuntu@10.0.1.51:~/solana 10.0.1.51
```

To run a performance-enhanced leader or validator (on Linux),
[CUDA 9.2](https://developer.nvidia.com/cuda-downloads) must be installed on
your system:
```bash
$ ./fetch-perf-libs.sh
$ SOLANA_CUDA=1 ./multinode-demo/leader.sh
$ SOLANA_CUDA=1 ./multinode-demo/validator.sh ubuntu@10.0.1.51:~/solana 10.0.1.51

```



Testnet Client Demo
---

Now that your singlenode or multinode testnet is up and running, in a separate shell, let's send it some transactions! Note we pass in
the JSON configuration file here, not the genesis ledger.

```bash
$ ./multinode-demo/client.sh ubuntu@10.0.1.51:~/solana 2 #The leader machine and the total number of nodes in the network
```

What just happened? The client demo spins up several threads to send 500,000 transactions
to the testnet as quickly as it can. The client then pings the testnet periodically to see
how many transactions it processed in that time. Take note that the demo intentionally
floods the network with UDP packets, such that the network will almost certainly drop a
bunch of them. This ensures the testnet has an opportunity to reach 710k TPS. The client
demo completes after it has convinced itself the testnet won't process any additional
transactions. You should see several TPS measurements printed to the screen. In the
multinode variation, you'll see TPS measurements for each validator node as well.

Linux Snap
---
A Linux [Snap](https://snapcraft.io/) is available, which can be used to
easily get Solana running on supported Linux systems without building anything
from source.  The `edge` Snap channel is updated daily with the latest
development from the `master` branch.  To install:
```bash
$ sudo snap install solana --edge --devmode
```
(`--devmode` flag is required only for `solana.fullnode-cuda`)

Once installed the usual Solana programs will be available as `solona.*` instead
of `solana-*`.  For example, `solana.fullnode` instead of `solana-fullnode`.

Update to the latest version at any time with
```bash
$ snap info solana
$ sudo snap refresh solana --devmode
```

### Daemon support
The snap supports running a leader, validator or leader+drone node as a system
daemon.

Run `sudo snap get solana` to view the current daemon configuration, and
`sudo snap logs -f solana` to view the daemon logs.

Disable the daemon at any time by running:
```bash
$ sudo snap set solana mode=
```

Runtime configuration files for the daemon can be found in
`/var/snap/solana/current/config`.

#### Leader daemon
```bash
$ sudo snap set solana mode=leader
```

If CUDA is available:
```bash
$ sudo snap set solana mode=leader enable-cuda=1
```

`rsync` must be configured and running on the leader.

1. Ensure rsync is installed with `sudo apt-get -y install rsync`
2. Edit `/etc/rsyncd.conf` to include the following
```
[config]
path = /var/snap/solana/current/config
hosts allow = *
read only = true
```
3. Run `sudo systemctl enable rsync; sudo systemctl start rsync`
4. Test by running `rsync -Pzravv rsync://<ip-address-of-leader>/config
solana-config` from another machine.  **If the leader is running on a cloud
provider it may be necessary to configure the Firewall rules to permit ingress
to port tcp:873, tcp:9900 and the port range udp:8000-udp:10000**


To run both the Leader and Drone:
```bash
$ sudo snap set solana mode=leader+drone

```

#### Validator daemon
```bash
$ sudo snap set solana mode=validator

```
If CUDA is available:
```bash
$ sudo snap set solana mode=validator enable-cuda=1
```

By default the validator will connect to **testnet.solana.com**, override
the leader IP address by running:
```bash
$ sudo snap set solana mode=validator leader-address=127.0.0.1 #<-- change IP address
```
It's assumed that the leader will be running `rsync` configured as described in
the previous **Leader daemon** section.

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

On Linux systems you may need to install libssl-dev and pkg-config.  On Ubuntu:
```bash
$ sudo apt-get install libssl-dev pkg-config
```

Download the source code:

```bash
$ git clone https://github.com/solana-labs/solana.git
$ cd solana
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

Debugging
---

There are some useful debug messages in the code, you can enable them on a per-module and per-level
basis with the normal RUST\_LOG environment variable. Run the fullnode with this syntax:
```bash
$ RUST_LOG=solana::streamer=debug,solana::server=info cat genesis.log | ./target/release/solana-fullnode > transactions0.log
```
to see the debug and info sections for streamer and server respectively. Generally
we are using debug for infrequent debug messages, trace for potentially frequent messages and
info for performance-related logging.

Attaching to a running process with gdb

```
$ sudo gdb
attach <PID>
set logging on
thread apply all bt
```

This will dump all the threads stack traces into gdb.txt

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
