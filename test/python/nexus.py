import pytest
import grpc
import mayastor_pb2 as pb
import mayastor_pb2_grpc as rpc
import logging

_LOGGER = logging.getLogger(__name__)
_LOGGER.setLevel(logging.INFO)

pytest_plugins = ["docker_compose"]

UUID = "0000000-0000-0000-0000-000000000001"
NEXUS_UUID = "3ae73410-6136-4430-a7b5-cbec9fe2d273"

# helper class to make it easier to create volumes and pools


class hdl(object):
    def __init__(self, bdev, ms):
        self.bdev = bdev
        self.ms = ms
        # dummy call to wait for connection
        self.bdev.List(pb.Null(), wait_for_ready=True)
        self.ms.ListPools(pb.Null(), wait_for_ready=True)

    def bdev_create(self, uri):
        return self.bdev.Create(pb.BdevUri(uri=uri))

    def pool_create(self, name, bdev):
        disks = []
        disks.append(bdev)
        return self.ms.CreatePool(pb.CreatePoolRequest(name=name,  disks=disks))

    def replica_create(self, pool, uuid, size):
        return self.ms.CreateReplica(pb.CreateReplicaRequest(pool=pool, uuid=uuid, size=size, thin=False, share=1))

    def nexus_create(self, uuid, size, children):
        return self.ms.CreateNexus(pb.CreateNexusRequest(uuid=uuid, size=size, children=children))

    def ms(self):
        return self.ms

# fixtures are a python thing, here we get handles to the gRPC servers
# the handles are valid through the life time of the test. We can also however
# tear things down for each test


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
        handles[n] = hdl(bdev, ms)
    return handles

# create pools using a memory disk for each


@pytest.fixture
def create_pools(wait_for_mayastor, create_bdevs):
    hdls = wait_for_mayastor
    pools = []
    pools.append(hdls["ms1"].pool_create(
        "tpool", "malloc:///disk0?size_mb=100"))
    pools.append(hdls["ms2"].pool_create(
        "tpool", "malloc:///disk0?size_mb=100"))
    for p in pools:
        assert p.state == pb.POOL_ONLINE
    return pools


# create the replica's on the pool

@pytest.fixture
def create_replica(wait_for_mayastor, create_pools):
    hdls = wait_for_mayastor
    pools = create_pools
    replicas = []
    replicas.append(hdls["ms1"].replica_create(
        pools[0].name, UUID, 64*1024 * 1024))
    replicas.append(hdls["ms2"].replica_create(
        pools[0].name, UUID, 64 * 1024 * 1024))
    print(replicas)
    return replicas

# create the nexus over the replica's. Note that the return values
# of the fixtures are passing this along. However, the wait_for_mayastor
# fixture is not. The values themselves however are cached


@pytest.fixture
def create_nexus(wait_for_mayastor, create_replica):
    replicas = create_replica
    hdls = wait_for_mayastor
    replicas = [k.uri for k in replicas]
    nexus = hdls["ms3"].nexus_create(NEXUS_UUID, 64*1024*1024, replicas)
    return nexus


def test_read_and_write(create_nexus):
    nexus = create_nexus
    print(nexus)
