<script lang="ts">
  let {
    name,
    size = 18,
    stroke = 1.7,
  }: {
    name: keyof typeof paths;
    size?: number;
    stroke?: number;
  } = $props();

  const paths = {
    overview: ['M3.5 3.5h7v7h-7z', 'M13.5 3.5h7v7h-7z', 'M3.5 13.5h7v7h-7z', 'M13.5 13.5h7v7h-7z'],
    approvals: ['M4 5h16v14H4z', 'M4 13h4l2 3h4l2-3h4'],
    nodes: [
      'M9 11a3.5 3.5 0 100-7 3.5 3.5 0 000 7z',
      'M2.5 20a6.5 6.5 0 0113 0',
      'M16.5 5a3 3 0 010 6',
      'M17.5 14.5a5.5 5.5 0 014 5.5',
    ],
    content: ['M6 3.5h8l4 4V20H6z', 'M14 3.5v4h4', 'M9 12h6', 'M9 16h6'],
    apps: ['M4 4h6v6H4z', 'M14 4h6v6h-6z', 'M4 14h6v6H4z', 'M14 14h6v6h-6z'],
    export: ['M12 3v12', 'M7 10l5 5 5-5', 'M4 20h16'],
    refresh: ['M20 11a8 8 0 10-2.3 6.7', 'M20 4v7h-7'],
    plus: ['M12 5v14', 'M5 12h14'],
    close: ['M6 6l12 12', 'M18 6L6 18'],
    copy: ['M9 9h11v11H9z', 'M15 5H4v11'],
    power: ['M12 3v9', 'M6.5 6.5a8 8 0 1011 0'],
    dots: ['M5 12h.01', 'M12 12h.01', 'M19 12h.01'],
    chevron: ['M9 6l6 6-6 6'],
    check: ['M5 12l4 4L19 6'],
    search: ['M11 19a8 8 0 100-16 8 8 0 000 16z', 'M21 21l-4.3-4.3'],
    shield: ['M12 3l7 3v6c0 4-3 7-7 9-4-2-7-5-7-9V6z'],
    alert: ['M12 4l9 16H3z', 'M12 9v5', 'M12 17h.01'],
  };
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each paths[name] as path}<path d={path} />{/each}
</svg>
