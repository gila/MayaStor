import pytest
import mayastor_pb2 as pb
from common.nvme import nvme_connect, nvme_disconnect, nvme_discover

UUID = "0000000-0000-0000-0000-000000000001"
NEXUS_UUID = "3ae73410-6136-4430-a7b5-cbec9fe2d273"


@pytest.fixture
def create_pools(wait_for_mayastor):
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
        pools[0].name, UUID, 64 * 1024 * 1024))
    replicas.append(hdls["ms2"].replica_create(
        pools[0].name, UUID, 64 * 1024 * 1024))
    return replicas

# create the nexus over the replica's. Note that the return values
# of the fixtures are passing this along. However, the wait_for_mayastor
# fixture is not. The values themselves however are cached


@pytest.fixture
def create_nexus(wait_for_mayastor, create_replica):
    replicas = create_replica
    hdls = wait_for_mayastor
    replicas = [k.uri for k in replicas]
    nexus = hdls["ms3"].nexus_create(NEXUS_UUID, 64 * 1024 * 1024, replicas)
    uri = hdls["ms3"].nexus_publish(NEXUS_UUID)

    assert len(hdls['ms2'].bdev_list().bdevs) == 2
    assert len(hdls['ms1'].bdev_list().bdevs) == 2
    assert len(hdls['ms3'].bdev_list().bdevs) == 1

    assert len(hdls['ms1'].pool_list().pools) == 1
    assert len(hdls['ms2'].pool_list().pools) == 1

    return (nexus, uri, hdls)


@pytest.fixture
def destroy_all(wait_for_mayastor):
    hdls = wait_for_mayastor

    hdls['ms3'].nexus_destroy(NEXUS_UUID)
    hdls['ms1'].replica_destroy(UUID)
    hdls['ms2'].replica_destroy(UUID)
    hdls['ms1'].pool_destroy("tpool")
    hdls['ms2'].pool_destroy("tpool")

    hdls['ms3'].nexus_destroy(NEXUS_UUID)
    hdls['ms1'].replica_destroy(UUID)
    hdls['ms2'].replica_destroy(UUID)
    hdls['ms1'].pool_destroy("tpool")
    hdls['ms2'].pool_destroy("tpool")

    assert len(hdls['ms1'].pool_list().pools) == 0
    assert len(hdls['ms2'].pool_list().pools) == 0

    assert len(hdls['ms2'].bdev_list().bdevs) == 0
    assert len(hdls['ms1'].bdev_list().bdevs) == 0
    assert len(hdls['ms3'].bdev_list().bdevs) == 0


@pytest.mark.parametrize("times", range(10))
def test_create_nexus_with_two_replica(times, create_nexus):
    nexus, uri, hdls = create_nexus
    nvme_discover(uri.device_uri)
    nvme_connect(uri.device_uri)
    nvme_disconnect(uri.device_uri)

    destroy_all
