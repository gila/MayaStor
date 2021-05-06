import mayastor_pb2 as pb
import grpc
import mayastor_pb2_grpc as rpc
import pytest

pytest_plugins = ["docker_compose"]


@pytest.fixture(scope="module")
def wait_for_mayastor(module_scoped_container_getter):
    handles = {}
    for n in ["ms1", "ms2", "ms3"]:
        services = module_scoped_container_getter.get(n)
        ip = services.get(
            'NetworkSettings.Networks.python_mayastor_net.IPAddress')
        channel = grpc.insecure_channel(("%s:10124") % ip)
        bdev = rpc.BdevRpcStub(channel)
        ms = rpc.MayastorStub(channel)
        handles[n] = MayastorHandle(bdev, ms)
    return handles


class MayastorHandle(object):
    def __init__(self, bdev, ms):
        self.bdev = bdev
        self.ms = ms
        # dummy call to wait for connection
        self.bdev_list()
        self.pool_list()

    def bdev_create(self, uri):
        return self.bdev.Create(pb.BdevUri(uri=uri))

    def pool_create(self, name, bdev):
        disks = []
        disks.append(bdev)
        return self.ms.CreatePool(pb.CreatePoolRequest(name=name, disks=disks))

    def pool_destroy(self, name):
        return self.ms.DestroyPool(pb.DestroyPoolRequest(name=name))

    def replica_create(self, pool, uuid, size):
        return self.ms.CreateReplica(pb.CreateReplicaRequest(
            pool=pool, uuid=uuid, size=size, thin=False, share=1))

    def replica_destroy(self, uuid):
        return self.ms.DestroyReplica(pb.DestroyReplicaRequest(uuid=uuid))

    def nexus_create(self, uuid, size, children):
        return self.ms.CreateNexus(pb.CreateNexusRequest(
            uuid=uuid, size=size, children=children))

    def nexus_destroy(self, uuid):
        return self.ms.DestroyNexus(pb.DestroyNexusRequest(uuid=uuid))

    def nexus_publish(self, uuid):
        return self.ms.PublishNexus(
            pb.PublishNexusRequest(uuid=uuid, key="", share=1))

    def nexus_unpublish(self, uuid):
        return self.ms.UnpublishNexus(pb.UnpublishNexusRequest(uuid=uuid))

    def bdev_list(self):
        return self.bdev.List(pb.Null(), wait_for_ready=True)

    def pool_list(self):
        return self.ms.ListPools(pb.Null(), wait_for_ready=True)
