# Node
## Installation
There is 2 methods of installation: 
- Docker (tested on Ubuntu 22.04)
- From source

### Docker
1. Update list of packages and install some need packages
```
sudo apt update
sudo apt install -y nano mc git curl
```
2. Install Docker

Actual instructions to [install](https://docs.docker.com/engine/install/ubuntu/#install-using-the-convenience-script) Docker

3. Clone this repo

```
git clone https://github.com/AploCoin/Node -b dev node
```
4. Edit  .env file.  Replace IP address in the ANNOUNCE_ADDRESS field to your
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
5. Build the docker container
``` docker build -t aplo_node:latest .```

### From source
