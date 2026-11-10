"""One-shot, read-only identification; sent inline after explicit SSH host-key trust.
No installation, sudo, network probes, shell evaluation or configuration writes.
"""
import json
import re
import socket
import subprocess
from pathlib import Path


def read(path):
    try:
        with open(path, 'rb') as stream:
            return stream.read(512).decode(errors='replace').strip('\0\n ')
    except OSError:
        return ''


def run(args):
    try:
        reply = subprocess.run(args, capture_output=True, timeout=3, text=True)
        return reply.stdout if reply.returncode == 0 and len(reply.stdout) <= 65536 else ''
    except (OSError, subprocess.TimeoutExpired):
        return ''


def decoded(text):
    try:
        return json.loads(text)
    except (ValueError, TypeError):
        return None


def field(value):
    value = value.get('value') if isinstance(value, dict) else value
    return value[:160] if isinstance(value, str) else ''


def neighbors(data):
    out = []
    lldp = data.get('lldp') if isinstance(data, dict) else None
    entries = lldp.get('interface') if isinstance(lldp, dict) else None
    entries = [entries] if isinstance(entries, dict) else entries if isinstance(entries, list) else []
    for entry in entries[:64]:
        if not isinstance(entry, dict):
            continue
        for name, info in entry.items():
            if not isinstance(info, dict):
                continue
            chassis, port = info.get('chassis') or {}, info.get('port') or {}
            if not isinstance(chassis, dict) or not isinstance(port, dict):
                continue
            if 'id' in chassis:
                system, body = field(chassis.get('name')), chassis
            else:
                system = next(iter(chassis), '')
                body = chassis.get(system) or {}
            if not isinstance(body, dict):
                continue
            out.append({'iface': str(name)[:64], 'system': str(system)[:160],
                        'chassis': field(body.get('id')), 'port': field(port.get('id')) or field(port.get('descr'))})
    return out[:64]


def other_neighbors(entries, host, interfaces):
    # Each ConnectX-7 half can hear its sibling through the switch. As in the
    # main collector, that is this node, not a neighbor at the far cable end.
    me = host.split('.')[0].lower()
    macs = {i['mac'].lower() for i in interfaces if i['mac']}
    return [n for n in entries if not (
        (me and n['system'].split('.')[0].lower() == me)
        or n['chassis'].lower() in macs or n['port'].lower() in macs)]


def collect():
    interfaces = []
    links = decoded(run(['ip', '-j', '-4', 'addr', 'show']))
    for link in (links if isinstance(links, list) else [])[:64]:
        if not isinstance(link, dict):
            continue
        name = link.get('ifname', '')
        if not isinstance(name, str) or not re.fullmatch(r'[A-Za-z0-9_.:-]{1,64}', name) or name == 'lo':
            continue
        base = Path('/sys/class/net') / name
        if not (base / 'device').exists():
            continue
        addresses = link.get('addr_info')
        addresses = addresses if isinstance(addresses, list) else []
        ips = [f"{a['local']}/{a['prefixlen']}" for a in addresses
               if isinstance(a, dict) and a.get('family') == 'inet' and isinstance(a.get('local'), str) and isinstance(a.get('prefixlen'), int)]
        speed = read(base / 'speed')
        # The IPv4-only ip report omits link-layer fields on DGX OS. Read the
        # address from the same physical interface, as the main collector does.
        interfaces.append({'name': name, 'mac': read(base / 'address')[:32], 'addresses': ips[:8],
                           'mtu': link.get('mtu') if isinstance(link.get('mtu'), int) else None,
                           'speedMbps': int(speed) if speed.isdigit() else None})
    host = socket.gethostname()[:160]
    return {'hostname': host, 'machineId': read('/etc/machine-id'),
            'model': read('/sys/class/dmi/id/product_name') or read('/proc/device-tree/model'),
            'gpu': run(['nvidia-smi', '--query-gpu=name', '--format=csv,noheader']).strip()[:160],
            'interfaces': interfaces, 'neighbors': other_neighbors(
                neighbors(decoded(run(['lldpcli', '-f', 'json', 'show', 'neighbors']))), host, interfaces)}


if __name__ == '__main__':
    print(json.dumps(collect()))
