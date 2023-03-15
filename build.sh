# simple build script for the docker container

docker build --build-arg PROFILE=release -t ghcr.io/friespascal/sero:0.1.0 .
docker push ghcr.io/friespascal/sero:0.1.0
