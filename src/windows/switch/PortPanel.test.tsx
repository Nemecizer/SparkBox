import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import type { SwitchSnapshot, SwitchPort } from "../../lib/types";
import { crs804Profile } from "../../lib/hardware";
import { PortPanel } from "./PortPanel";
const port = (c: number, suffix = 1, node = `cell${c}`): SwitchPort => ({ name:`qsfp56-dd-${c}-${suffix}`,node,comment:node,running:true,disabled:false,speed:"200G",capacityMbps:200000,rxBps:0,txBps:0,rxErrors:0,txErrors:0,rxDrops:0,txDrops:0,rxPause:0,txPause:0,linkDowns:0,fcsErrors:0,rxPowerDbm:-2,txPowerDbm:0,moduleTempC:30 } as SwitchPort);
const sw = (ports: SwitchPort[], board: string | null = "CRS804-4DDQ"): SwitchSnapshot => ({ id:"crs",name:"Fabric switch",kind:"routerOs",host:"fixture",status:"online",latencyMs:1,version:"7",cpuPct:1,tempC:40,uptimeS:1,ports,board });
const props = { selected:null, onSelect:vi.fn(),rising:()=>false,nodeName:(id:string|null|undefined)=>id??null };
describe("physical switch layout", () => {
  it("draws four occupied sockets with observed 200G links and preserves the real cable order", () => {
    const { container }=render(<PortPanel {...props} sw={sw([port(1,1,"cell1"),port(4,1,"cell2"),port(2,1,"cell3"),port(3,1,"cell4")])}/>);
    expect(container.querySelectorAll("[data-physical-socket]")).toHaveLength(4);
    expect(screen.getByText(/4 \/ 4 sockets active/)).toBeInTheDocument();
    expect(screen.getAllByText("200G")).toHaveLength(4);
    const fourth=container.querySelector('[data-physical-socket="4"]')!;
    expect(fourth).toHaveTextContent("cell2"); fireEvent.click(fourth); expect(props.onSelect).toHaveBeenCalledWith("qsfp56-dd-4-1");
    expect(screen.getByText("MGMT 1")).toBeInTheDocument(); expect(screen.getByText("MGMT 2")).toBeInTheDocument();
  });
  it("eight logical breakout interfaces still produce four physical sockets", () => {
    const {container}=render(<PortPanel {...props} sw={sw([1,2,3,4].flatMap(c=>[port(c,1),port(c,2,`peer${c}`)]))}/>);
    expect(container.querySelectorAll("[data-physical-socket]")).toHaveLength(4);
    expect(screen.getByText(/4 \/ 4 sockets active/)).toBeInTheDocument();
    expect(screen.getAllByText("200G + 200G")).toHaveLength(4);
  });
  it("supports unsplit names and keyboard selection without inventing unused links", () => {
    const p={...port(2),name:"qsfp56-dd-2",speed:"400G"};
    const {container}=render(<PortPanel {...props} sw={sw([p])}/>);
    expect(screen.getByText(/1 \/ 4 sockets active/)).toBeInTheDocument();
    fireEvent.keyDown(container.querySelector('[data-physical-socket="2"]')!,{key:"Enter"});
    expect(props.onSelect).toHaveBeenCalledWith("qsfp56-dd-2");
  });
  it("does not show the CRS804 chassis for another model or an unknown board", () => {
    const {container,rerender}=render(<PortPanel {...props} sw={sw([port(1)],null)}/>);
    expect(screen.getByText("Physical layout not identified")).toBeInTheDocument();
    expect(container.querySelectorAll("[data-physical-socket]")).toHaveLength(0);
    rerender(<PortPanel {...props} sw={sw([],"Different model")} profile={{...crs804Profile,model:"Different model"}}/>);
    expect(screen.getByRole("img",{name:/4 physical QSFP56-DD sockets/})).toBeInTheDocument();
    expect(screen.queryByText("CRS804 DDQ")).not.toBeInTheDocument();
  });
});
