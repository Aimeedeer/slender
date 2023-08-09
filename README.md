# Slender

The Slender lending protocol uses a pool-based strategy that aggregates each user's supplied assets. Users will be able to provide some assets to the protocol and earn interest.

When users supply liquidity they get LP tokens or as we call them sTokens in return. sTokens accrue interest and reflect this accrual in their “price”.

## Build

### Prerequisites

To build and run unit tests you need to install **rust**. See https://soroban.stellar.org/docs/getting-started/setup

For building all packages run:

```shell
make
```

## Unit Tests

In order to run unit tests use command below:

```shell
make test
```

## Deploy and run integration tests

To run the tests you need to install **soroban-cli** version 0.9.1

```shell
cargo install --locked --version 0.9.1 soroban-cli
```

Run a local standalone (or Futurenet) network with the following command:

```shell
# Local environment
docker run --rm -it \
  -p 8000:8000 \
  --name stellar \
  stellar/quickstart:soroban-dev@sha256:a6b03cf6b0433c99f2f799b719f0faadbb79684b1b763e7674ba749fb0f648ee \
  --standalone \
  --enable-soroban-rpc

# Futurenet (note, you must wait for synchronization)
docker run --rm -it \
   -p 8000:8000 \
   --name stellar \
   stellar/quickstart:soroban-dev@sha256:ed57f7a7683e3568ae401f5c6e93341a9f77d8ad41191bf752944d7898981e0c \
   --futurenet \
   --enable-soroban-rpc
```

Run the tests from the root project directory:

```shell
# Local
make integration-test env="develop"

# Futurenet
make integration-test env="futurenet"
```

Note, all the integration tests parameters can be found in `integration-tests/.${environment}.env`.

### How to add token to freighter wallet

In order to add new token to the freighter wallet you need to convert token address base32 representation to hex and use it as Token Id.

Script example:

```js
const { StrKey } = require("soroban-client");

let id = "CCLZ4QF5QSWBANABDZPC3XKHMVX3GUSLFLP4JS22SCIGLADD2E7STHDR";
console.log(StrKey.decodeContract(id).toString("hex"));
//result
//979e40bd84ac1034011e5e2ddd47656fb3524b2adfc4cb5a9090658063d13f29
```
