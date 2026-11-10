import json
import unittest
from unittest.mock import patch
import read

class DiscoveryReadTests(unittest.TestCase):
    def test_lldp_dict_and_list_preserve_only_reported_links(self):
        entry = {'eth0': {'chassis': {'switch': {'id': {'value': 'aa:bb'}}}, 'port': {'id': {'value': 'port4'}}}}
        for value in [entry, [entry]]:
            self.assertEqual(read.neighbors({'lldp': {'interface': value}}), [{'iface': 'eth0', 'system': 'switch', 'chassis': 'aa:bb', 'port': 'port4'}])
        self.assertEqual(read.neighbors(None), [])
        self.assertEqual(read.neighbors({'lldp': []}), [])
        self.assertEqual(read.neighbors({'lldp': {'interface': [None, {'eth0': []}]}}), [])

    def test_reply_limits_and_no_invented_neighbors(self):
        entries = [{f'eth{i}': {'chassis': {'switch': {}}, 'port': {'id': 'x'*1000}}} for i in range(100)]
        rows = read.neighbors({'lldp': {'interface': entries}})
        self.assertEqual(len(rows), 64)
        self.assertEqual(len(rows[0]['port']), 160)
        self.assertEqual(rows[0]['chassis'], '')

    def test_reflected_local_interfaces_are_not_remote_neighbors(self):
        def entry(system='', chassis='', port=''):
            return {'iface': 'enp1', 'system': system, 'chassis': chassis, 'port': port}
        switch = entry('CRS804', '02:00:00:99:00:01', 'qsfp56-dd-1-1')
        other_node = entry('cell2-example.lan', port='02:00:00:00:00:02')
        echoes = [entry('CELL1-EXAMPLE.lan'), entry(chassis='02:00:00:00:AA:01'),
                  entry(port='02:00:00:00:AA:01')]
        interfaces = [{'mac': '02:00:00:00:aa:01'}, {'mac': ''}]
        self.assertEqual(read.other_neighbors(echoes + [switch, other_node], 'cell1-example', interfaces), [switch, other_node])
        self.assertEqual(read.other_neighbors(echoes, 'cell1-example', interfaces), [])
        self.assertEqual(read.other_neighbors([switch], '', []), [switch])

    def test_collect_uses_only_fixed_read_commands_and_physical_interfaces(self):
        commands = []
        def fake(args):
            commands.append(args)
            if args[0] == 'ip': return '[null, {"ifname":"lo"}, {"ifname":"../../oops"}, {"ifname":"enp1", "address":"02:00:00:00:00:01", "addr_info":[null,{"family":"inet","local":"192.168.1.2","prefixlen":24}]}]'
            if args[0] == 'nvidia-smi': return 'NVIDIA GB10'
            return ''
        with patch.object(read, 'run', side_effect=fake), patch.object(read, 'read', return_value=''), patch.object(read.Path, 'exists', return_value=True):
            f = read.collect()
        self.assertEqual(f['neighbors'], [])
        self.assertEqual([x['name'] for x in f['interfaces']], ['enp1'])
        self.assertEqual(f['interfaces'][0]['addresses'], ['192.168.1.2/24'])
        self.assertEqual(commands, [['ip','-j','-4','addr','show'], ['nvidia-smi','--query-gpu=name','--format=csv,noheader'], ['lldpcli','-f','json','show','neighbors']])

    def test_missing_tools_and_malformed_json_fail_closed(self):
        with patch.object(read.subprocess, 'run', side_effect=FileNotFoundError):
            self.assertEqual(read.run(['missing']), '')
        self.assertIsNone(read.decoded('not json'))

    def test_ipv4_only_report_reads_mac_from_the_same_physical_interface(self):
        # ip -j -4 addr show on the Sparks omits the link-layer address.
        links = [{'ifname': 'enP7s7', 'mtu': 1500, 'addr_info': [
            {'family': 'inet', 'local': '192.168.50.51', 'prefixlen': 24}]},
            {'ifname': 'docker0', 'addr_info': [
                {'family': 'inet', 'local': '172.17.0.1', 'prefixlen': 16}]}]
        files = {'/etc/machine-id': '1' * 32,
                 '/sys/class/net/enP7s7/address': '02:00:00:00:00:51',
                 '/sys/class/net/enP7s7/speed': '2500'}
        commands = []
        def run(args):
            commands.append(args)
            return json.dumps(links) if args[0] == 'ip' else 'NVIDIA GB10' if args[0] == 'nvidia-smi' else ''
        with patch.object(read, 'run', side_effect=run), \
             patch.object(read, 'read', side_effect=lambda p: files.get(str(p), '')), \
             patch.object(read.Path, 'exists', autospec=True, side_effect=lambda p: str(p) == '/sys/class/net/enP7s7/device'), \
             patch.object(read.socket, 'gethostname', return_value='cell1-example'):
            facts = read.collect()
            self.assertEqual(facts['interfaces'], [{'name': 'enP7s7', 'mac': '02:00:00:00:00:51',
                'addresses': ['192.168.50.51/24'], 'mtu': 1500, 'speedMbps': 2500}])
            self.assertEqual(facts['machineId'], '1' * 32)
            self.assertEqual(facts['gpu'], 'NVIDIA GB10')
            # Missing MAC evidence stays missing, so the Rust identity gate can refuse it.
            del files['/sys/class/net/enP7s7/address']
            self.assertEqual(read.collect()['interfaces'][0]['mac'], '')
        self.assertEqual(commands, [['ip', '-j', '-4', 'addr', 'show'],
            ['nvidia-smi', '--query-gpu=name', '--format=csv,noheader'],
            ['lldpcli', '-f', 'json', 'show', 'neighbors']] * 2)

if __name__ == '__main__': unittest.main()
