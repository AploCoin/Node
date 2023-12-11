# Node

## Installation
There is 2 methods of installation: 
- Docker (tested on Ubuntu 22.04)
- From source (tested on Ubuntu 18.04, Debian 12, Termux 0.118.0, Alpine 3.18) 

### From source

#### Debian/Ubuntu
1. Update list of packages and install some needed packages
```
sudo apt update
sudo apt install -y curl git build-essential binutils
```
2. Install Rust
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
3. Clone this repo

```
git clone https://github.com/AploCoin/Node -b dev node
```
4. Edit  .env file.  Replace IP address in the ANNOUNCE_ADDRESS field to your
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
5. Build node
```
cargo build --release
```
6. Run node
```
cargo run --release
```

#### Alpine
1. Update list of packages and install rust, cargo and git
```
apk update
apk install git rust cargo
```
2. Clone this repo
```
git clone https://github.com/AploCoin/Node -b dev node
```
3. Edit  .env file.  Replace IP address in the ANNOUNCE_ADDRESS field to yours
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
4. Build node
 ```
cargo build --release
```
5. Run node
```
cargo run --release
```
