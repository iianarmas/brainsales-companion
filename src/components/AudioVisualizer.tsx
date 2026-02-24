import { useRef, useEffect } from "react";

interface AudioVisualizerProps {
  micLevel: number;
  sysLevel: number;
}

export function AudioVisualizer({ micLevel, sysLevel }: AudioVisualizerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let animationFrameId: number;
    
    const draw = () => {
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      
      const bars = 40;
      const spacing = 4;
      const barWidth = (canvas.width - (bars - 1) * spacing) / bars;
      
      for (let i = 0; i < bars; i++) {
        // Procedural organic movement + live audio input
        const isMic = i % 2 === 0;
        const baseLevel = isMic ? micLevel : sysLevel;
        
        // Higher sensitivity and smoother noise
        const time = Date.now() * 0.005;
        const noise = Math.sin(time + i * 0.2) * 0.1 + 0.1;
        const targetHeight = (baseLevel * 1.5 + noise) * canvas.height * 0.7;
        
        const x = i * (barWidth + spacing);
        const height = Math.max(4, targetHeight);
        const y = (canvas.height - height) / 2;

        // Gradient for a premium look
        ctx.save();
        const gradient = ctx.createLinearGradient(x, y, x, y + height);
        if (isMic) {
          gradient.addColorStop(0, "#818cf8"); // Indigo-400
          gradient.addColorStop(1, "#6366f1"); // Indigo-500
          ctx.shadowColor = "rgba(99, 102, 241, 0.4)";
        } else {
          gradient.addColorStop(0, "#34d399"); // Emerald-400
          gradient.addColorStop(1, "#10b981"); // Emerald-500
          ctx.shadowColor = "rgba(16, 185, 129, 0.4)";
        }

        ctx.shadowBlur = baseLevel > 0.05 ? 12 : 0;
        ctx.fillStyle = gradient;
        
        // Use roundRect if available (modern browsers)
        if ('roundRect' in ctx) {
            // @ts-ignore
            ctx.beginPath();
            // @ts-ignore
            ctx.roundRect(x, y, barWidth, height, barWidth / 2);
            ctx.fill();
        } else {
            // Fallback rounded bar
            ctx.beginPath();
            const radius = barWidth / 2;
            ctx.moveTo(x, y + radius);
            ctx.lineTo(x, y + height - radius);
            ctx.arcTo(x, y + height, x + radius, y + height, radius);
            ctx.lineTo(x + barWidth - radius, y + height);
            ctx.arcTo(x + barWidth, y + height, x + barWidth, y + height - radius, radius);
            ctx.lineTo(x + barWidth, y + radius);
            ctx.arcTo(x + barWidth, y, x + barWidth - radius, y, radius);
            ctx.lineTo(x + radius, y);
            ctx.arcTo(x, y, x, y + radius, radius);
            ctx.fill();
        }
        ctx.restore();
      }
      
      animationFrameId = requestAnimationFrame(draw);
    };

    draw();
    return () => cancelAnimationFrame(animationFrameId);
  }, [micLevel, sysLevel]);

  return (
    <canvas 
      ref={canvasRef} 
      className="visualizer-canvas"
      width={280} 
      height={80} 
    />
  );
}
