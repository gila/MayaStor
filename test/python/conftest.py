
import grpc
import mayastor_pb2_grpc as rpc
import pytest
from common.hdl import MayastorHandle

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
