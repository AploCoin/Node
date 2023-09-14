# Node
*  [Переключить на Русский](https://github.com/AploCoin/Node/blob/dev/README_ru.md)
## Installation
There is 2 methods of installation: 
- Docker (tested on Ubuntu 22.04)
- From source (tested on Ubuntu 18.04, Debian 12, Termux 0.118.0) 

### Docker
1. Update list of packages and install some need packages
```
sudo apt update
sudo apt install -y nano mc git curl
```
2. Install Docker

Actual instructions to [install](https://docs.docker.com/engine/install/ubuntu/) Docker

3. Clone this repo

```
git clone https://github.com/AploCoin/Node -b dev node
```
4. Edit  .env file.  Replace IP address in the ANNOUNCE_ADDRESS field to your
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
5. Build the docker container
```
docker build -t aplo_node:latest .
```
6. Run container
```
mkdir logs
docker run --network host --name aplo_node -v logs:/Node/logs aplo_node
```

### From source
1. Update list of packages and install some need packages
```
sudo apt update
sudo apt install -y curl git nano build-essential binutils
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
