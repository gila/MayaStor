from urllib.parse import urlparse
from subprocess import run


def nvme_connect(uri):
    u = urlparse(uri)
    port = u.port
    host = u.hostname
    nqn = u.path[1:]

    command = "sudo nvme connect -t tcp -s {0} -a {1} -n {2}".format(
        port, host, nqn)
    run(command, check=True, shell=True, capture_output=False)
    print("connected to {}".format(uri))


def nvme_discover(uri):
    u = urlparse(uri)
    port = u.port
    host = u.hostname

    command = "sudo nvme discover -t tcp -s {0} -a {1}".format(
        port, host)
    output = run(
        command,
        check=True,
        shell=True,
        capture_output=True,
        encoding="utf-8")
    print(output.stdout)
    if not u.path[1:] in str(output.stdout):
        raise ValueError("uri {} is not discovered".format(u.path[1:]))


def nvme_disconnect(uri):
    u = urlparse(uri)
    nqn = u.path[1:]

    command = "sudo nvme disconnect -n {0}".format(nqn)
    run(command, check=True, shell=True, capture_output=True)
