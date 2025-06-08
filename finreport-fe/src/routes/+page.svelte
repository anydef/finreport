<script lang="ts">
    import {onMount} from 'svelte';
    import Chart from 'chart.js/auto';
    export let data: { payload: { year: string; count: number }[] };
    let canvas: HTMLCanvasElement;
    // let {data}: any = $props();
    onMount(() => {
        const ctx = canvas.getContext('2d');
        console.log(data.payload);
        const config = {
            type: 'bar',
            data: {
                labels: data.payload.map(row => row.year),
                datasets: [
                    {
                        label: 'Acquisitions by year',
                        data: data.payload.map(row => row.count)
                    }
                ]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false
            }
        };

        new Chart(ctx, config);
    });
</script>

<style>
    canvas {
        width: 400px;
        height: 400px;
    }
</style>

<h1>Welcome to SvelteKit</h1>
<p>Visit <a href="https://svelte.dev/docs/kit">svelte.dev/docs/kit</a> to read the documentation</p>

<div>
    <canvas bind:this={canvas} id="myChart"></canvas>
</div>