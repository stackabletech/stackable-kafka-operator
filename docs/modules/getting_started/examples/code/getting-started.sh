#!/usr/bin/env bash
set -euo pipefail

# The getting started guide script
# It uses tagged regions which are included in the documentation
# https://docs.asciidoctor.org/asciidoc/latest/directives/include-tagged-regions/
#
# There are two variants to go through the guide - using stackablectl or helm
# The script takes either 'stackablectl' or 'helm' as an argument
#
# The script can be run as a test as well, to make sure that the tutorial works
# It includes some assertions throughout, and at the end especially.

if [ $# -eq 0 ]
then
  echo "Installation method argument ('helm' or 'stackablectl') required."
  exit 1
fi

case "$1" in
"helm")
echo "Adding 'stackable-dev' Helm Chart repository"
# tag::helm-add-repo[]
helm repo add stackable-dev https://repo.stackable.tech/repository/helm-dev/
# end::helm-add-repo[]
echo "Installing Operators with Helm"
# tag::helm-install-operators[]
helm install --wait commons-operator stackable-dev/commons-operator --version 0.5.0-nightly
helm install --wait secret-operator stackable-dev/secret-operator --version 0.6.0-nightly
helm install --wait zookeeper-operator stackable-dev/zookeeper-operator --version 0.13.0-nightly
helm install --wait kafka-operator stackable-dev/kafka-operator --version 0.9.0-nightly
# end::helm-install-operators[]
;;
"stackablectl")
echo "installing Operators with stackablectl"
# tag::stackablectl-install-operators[]
stackablectl operator install \
  commons=0.5.0-nightly \
  secret=0.6.0-nightly \
  zookeeper=0.13.0-nightly \
  kafka=0.9.0-nightly
# end::stackablectl-install-operators[]
;;
*)
echo "Need to provide 'helm' or 'stackablectl' as an argument for which installation method to use!"
exit 1
;;
esac

echo "Installing ZooKeeper from zookeeper.yaml"
# tag::install-zookeeper[]
kubectl apply -f zookeeper.yaml
# end::install-zookeeper[]

echo "Installing ZNode from kafka-znode.yaml"
# tag::install-znode[]
kubectl apply -f kafka-znode.yaml
# end::install-znode[]

sleep 5

echo "Awaiting ZooKeeper rollout finish"
# tag::watch-zookeeper-rollout[]
kubectl rollout status --watch statefulset/simple-zk-server-default
# end::watch-zookeeper-rollout[]

echo "Install KafkaCluster from kafka.yaml"
# tag::install-kafka[]
kubectl apply -f kafka.yaml
# end::install-kafka[]

sleep 5

echo "Awaiting Kafka rollout finish"
# tag::watch-kafka-rollout[]
kubectl rollout status --watch statefulset/simple-kafka-broker-default
# end::watch-kafka-rollout[]

echo "Starting port-forwarding of port 9092"
# tag::port-forwarding[]
kubectl port-forward svc/simple-kafka 9092 2>&1 >/dev/null &
# end::port-forwarding[]
PORT_FORWARD_PID=$!
trap "kill $PORT_FORWARD_PID" EXIT

sleep 5

echo "Creating test data"
# tag::kcat-create-data[]
echo "some test data" > data
# end::kcat-create-data[]

echo "Writing test data"
# tag::kcat-write-data[]
kafkacat -b localhost:9092 -t test-data-topic -P data
# end::kcat-write-data[]

echo "Reading test data"
# tag::kcat-read-data[]
kafkacat -b localhost:9092 -t test-data-topic -C -e > read-data
# end::kcat-read-data[]

echo "Check contents"
# tag::kcat-check-data[]
cat read-data | grep "some test data"
# end::kcat-check-data[]

echo "Cleanup"
# tag::kcat-cleanup-data[]
rm data
rm read-data
# end::kcat-cleanup-data[]
