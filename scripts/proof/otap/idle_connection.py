"""Hold a gRPC connection open without ever making a call.

Used by proof.sh: connect to the port given, send the HTTP/2 preface and
an empty SETTINGS frame, then only read. Prints the whole seconds until
the server closed the connection, or "open" when it is still there
after a minute.
"""
import socket
import sys
import time

PREFACE = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
EMPTY_SETTINGS = b"\x00\x00\x00\x04\x00\x00\x00\x00\x00"


def held_seconds(port: int) -> str:
    connection = socket.create_connection(("127.0.0.1", port))
    start = time.monotonic()
    connection.sendall(PREFACE + EMPTY_SETTINGS)
    connection.settimeout(60)
    try:
        while connection.recv(4096):
            pass
    except socket.timeout:
        return "open"
    except OSError:
        pass
    return str(round(time.monotonic() - start))


if __name__ == "__main__":
    print(held_seconds(int(sys.argv[1])))
