---
id: read-the-charts
title: Read the charts
type: howto
area: monitor
summary: Change the time range, read exact values, use the log scale and copy a chart's data.
keywords: [chart, graph, history, time range, log scale, zoom, csv, peaks]
verified: false
related: [node-window, cluster-metrics, dashboard]
---

# Read the charts

To see what a node or the cluster did over time, choose a time range on a chart, then move the pointer over a moment to read its values.

## Before you start

- You don't need Admin mode. Charts only read.
- **Demo** data fills the charts with simulated history, so it's a safe place to practice.
- The charts work the same way in a node's window, on the **Dashboard** and in **Cluster metrics**. The **Dashboard** always shows the last 15 minutes.

## Steps

### Change the time range

1. In the sidebar, under **Nodes**, click a node such as `cell1`. The node's window opens on its **Overview** tab, with a bar of ranges above the charts.
2. Click a range: **5m**, **15m**, **1h**, **6h**, **24h** or **7d**. Every chart on the tab redraws for that range.

Ranges up to **1h** show a reading for every second. **6h** and longer come from saved history, so each point is an average, with the lowest and highest readings drawn as a faint band.

### Read exact values

1. Move the pointer over a chart. A vertical line follows it, and the chart's heading shows the time and each value at that moment. A narrow chart with a legend leaves the time out, and the line still marks it.
2. Look at the other charts on the tab. The same moment is marked on them too, so you can compare them.

### Zoom in

1. Drag across a chart, from the start of the stretch you care about to its end. Every chart on the tab zooms to that stretch. The bar above them reads **Paused**, with the times.
2. To go back, click **Back to live**, or double-click the chart.

On the **Dashboard**, **Fabric throughput** and **GPU power** zoom one at a time. **GPU utilization** and **Unified memory** don't zoom there.

### Show or hide a line

1. Click a name in a chart's legend, such as **CPU**. That line disappears. Click again to bring it back.
2. To see one line alone, hold <kbd>⇧</kbd> and click its name. Do the same again to show them all.

### See tall peaks

Some traffic comes in short, tall bursts. So that one burst doesn't flatten the rest, these charts fit their axis to the steady traffic. The bursts sit on the top edge with ▲, and a chip names the highest.

1. To see everything to scale, click **Log** in the chart's heading. The axis changes to a logarithmic scale, where each step up is a multiple, not a fixed amount. **Log** appears only on charts that have bursts.
2. Click **Log** again to go back.

### Copy the data

1. Right-click the chart, or click its **More** button (the three dots in its heading). A menu opens.
2. Click **Copy data as CSV**. A message confirms how many rows were copied.
3. Paste the data into a spreadsheet. CSV is plain text with commas between the values.

## What you should see

- **Dashed lines with a chip** mark a limit. For example, the **Unified memory** chart marks 90%. A limit turns amber or red once a reading reaches it. Two limits close together, such as 90% and 95%, each carry their chip on their own line.
- **Small marks along the top** of some charts show events: alerts that fired and changes made to the cluster. Move the pointer over one to read it.
- **A band reading "No data recorded"**, or a break in a line, marks missing readings. SparkBox may have been closed, this Mac may have slept, or a node may have been reconnecting. The gap alone does not tell you why. SparkBox leaves missing readings blank; it fills them only when an [always-on agent](help:always-on-agent) supplies actual records.
- **"History starts"** marks when recording began.

## Why it works

SparkBox reads every node once a second and keeps the last hour of those readings in full. Older readings are combined into longer steps, each with its lowest, average and highest value. That's why long ranges show a band: the line is the average, and the band shows how far readings strayed from it. Live and Demo keep separate history.

## Limits

- Charts go back 7 days at most, even though SparkBox keeps more.
- A zoomed chart stops following the live data until you go back.

## If something goes wrong

- **"Couldn't copy the chart data".** The clipboard wasn't available. Click the chart again, then try once more.
- **A chart is empty or shows only a band.** SparkBox has no readings for that time. Choose a shorter range, or wait for readings to arrive.
- **A value reads as a dash.** That node isn't reporting. See [A node shows Offline](help:troubleshooting#a-node-shows-offline).

## Next steps

- [A node's window](help:node-window), to see what each chart measures
- [Cluster metrics](help:cluster-metrics), to compare every node side by side
- [Read the Dashboard](help:read-the-dashboard)
