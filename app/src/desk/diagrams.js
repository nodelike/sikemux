(() => {
  const root = getComputedStyle(document.documentElement);
  const token = (name) => root.getPropertyValue(name).trim() || undefined;
  for (const code of document.querySelectorAll('pre > code.language-mermaid')) {
    const pre = code.parentElement;
    pre.textContent = code.textContent;
    pre.classList.add('mermaid');
  }
  const ground = token('--background');
  mermaid.initialize({
    startOnLoad: false,
    theme: 'base',
    darkMode: root.colorScheme === 'dark',
    fontFamily: token('--font-sans'),
    themeVariables: {
      background: ground === 'transparent' ? undefined : ground,
      fontFamily: token('--font-sans'),
      primaryColor: token('--card'),
      primaryTextColor: token('--foreground'),
      primaryBorderColor: token('--border'),
      secondaryColor: token('--accent-surface'),
      tertiaryColor: token('--code-background'),
      lineColor: token('--muted-foreground'),
      textColor: token('--foreground'),
    },
  });

  const zoomable = (diagram) => {
    const svg = diagram.querySelector('svg');
    if (!svg) return;
    let scale = 1;
    let x = 0;
    let y = 0;
    const draw = () => {
      svg.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
      diagram.style.cursor = scale > 1 ? 'grab' : '';
    };
    const zoomAt = (clientX, clientY, by) => {
      const next = Math.min(8, Math.max(1, scale * by));
      const box = diagram.getBoundingClientRect();
      const px = clientX - box.left;
      const py = clientY - box.top;
      x = px - ((px - x) * next) / scale;
      y = py - ((py - y) * next) / scale;
      scale = next;
      if (scale === 1) x = y = 0;
      draw();
    };
    svg.style.transformOrigin = '0 0';
    diagram.style.overflow = 'hidden';
    diagram.title = 'Pinch or ⌘-scroll to zoom, drag to move, double-click to fit';
    diagram.addEventListener(
      'wheel',
      (event) => {
        if (event.ctrlKey || event.metaKey) {
          event.preventDefault();
          zoomAt(event.clientX, event.clientY, Math.exp(-event.deltaY / 200));
        } else if (scale > 1) {
          event.preventDefault();
          x -= event.deltaX;
          y -= event.deltaY;
          draw();
        }
      },
      { passive: false },
    );
    let pinch = null;
    diagram.addEventListener('gesturestart', (event) => {
      event.preventDefault();
      pinch = { scale: 1, clientX: event.clientX, clientY: event.clientY };
    });
    diagram.addEventListener('gesturechange', (event) => {
      if (!pinch) return;
      event.preventDefault();
      zoomAt(pinch.clientX, pinch.clientY, event.scale / pinch.scale);
      pinch.scale = event.scale;
    });
    diagram.addEventListener('gestureend', () => (pinch = null));
    diagram.addEventListener('pointerdown', (event) => {
      if (scale === 1 || event.button !== 0) return;
      const start = { x: event.clientX - x, y: event.clientY - y };
      diagram.setPointerCapture(event.pointerId);
      diagram.style.cursor = 'grabbing';
      const move = (moved) => {
        x = moved.clientX - start.x;
        y = moved.clientY - start.y;
        draw();
      };
      const up = () => {
        diagram.removeEventListener('pointermove', move);
        draw();
      };
      diagram.addEventListener('pointermove', move);
      diagram.addEventListener('pointerup', up, { once: true });
      diagram.addEventListener('pointercancel', up, { once: true });
    });
    diagram.addEventListener('dblclick', () => {
      scale = 1;
      x = y = 0;
      draw();
    });
  };

  document.fonts.ready
    .then(() => mermaid.run({ querySelector: '.mermaid' }))
    .catch((error) => console.error('A diagram could not be drawn:', error))
    .finally(() => document.querySelectorAll('.mermaid').forEach(zoomable));
})();
