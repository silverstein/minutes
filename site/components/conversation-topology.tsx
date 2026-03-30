'use client';

import { useRef, useMemo, useEffect, useState, useCallback } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import * as THREE from 'three';

// ---------------------------------------------------------------------------
// Quality detection (Shopify 4-tier pattern)
// ---------------------------------------------------------------------------
type QualityTier = 'high' | 'medium' | 'low' | 'fallback';

function detectQualityTier(): QualityTier {
  if (typeof window === 'undefined') return 'fallback';
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return 'fallback';

  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2') || canvas.getContext('webgl');
  if (!gl) return 'fallback';

  const debugInfo = gl.getExtension('WEBGL_debug_renderer_info');
  const renderer = debugInfo ? gl.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL) : '';
  const isLowPower = /Intel|Mali|Adreno 3|Adreno 4/i.test(renderer);
  const isMobile = /Android|iPhone|iPad/i.test(navigator.userAgent);

  if (isLowPower && isMobile) return 'low';
  if (isLowPower || isMobile) return 'medium';
  return 'high';
}

// ---------------------------------------------------------------------------
// Generate ribbon curve — two speakers weaving through space
// The curves are designed so speakers CROSS OVER each other at specific
// points (interruptions, agreements) and diverge during monologues.
// ---------------------------------------------------------------------------
function generateSpeakerCurve(
  speakerIndex: number,
  pointCount: number,
): THREE.CatmullRomCurve3 {
  const points: THREE.Vector3[] = [];
  const side = speakerIndex === 0 ? -1 : 1;

  for (let i = 0; i < pointCount; i++) {
    const t = i / (pointCount - 1);

    // Primary weave — slow, dramatic crossovers
    const crossover = Math.sin(t * Math.PI * 2.5) * 1.0;
    // Secondary flutter — faster, smaller
    const flutter = Math.sin(t * Math.PI * 7.0 + speakerIndex * 1.2) * 0.15;
    // Moments of convergence (speakers close together) vs divergence
    const separation = 0.25 + Math.abs(Math.sin(t * Math.PI * 1.8)) * 0.4;

    const x = side * separation + crossover * 0.6 + flutter;

    // Vertical: flows top to bottom
    const y = (0.5 - t) * 5.5;

    // Depth: subtle z for parallax
    const z = Math.sin(t * Math.PI * 2.2 + speakerIndex * 2.0) * 0.4;

    points.push(new THREE.Vector3(x, y, z));
  }

  return new THREE.CatmullRomCurve3(points, false, 'catmullrom', 0.5);
}

// ---------------------------------------------------------------------------
// Ribbon shader — desaturated, foggy, atmospheric
// ---------------------------------------------------------------------------
const ribbonVertexShader = /* glsl */ `
  uniform float uTime;
  uniform float uQuality;
  varying vec2 vUv;
  varying float vProgress;
  varying vec3 vNormal;
  varying vec3 vWorldPos;

  void main() {
    vUv = uv;
    vProgress = uv.x;
    vNormal = normalize(normalMatrix * normal);

    vec3 pos = position;
    if (uQuality > 0.5) {
      float breathe = sin(uTime * 0.6 + uv.x * 10.0) * 0.008;
      pos += normal * breathe;
    }

    vWorldPos = (modelMatrix * vec4(pos, 1.0)).xyz;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(pos, 1.0);
  }
`;

const ribbonFragmentShader = /* glsl */ `
  uniform float uTime;
  uniform float uScrollFade;
  uniform vec3 uColorA;
  uniform vec3 uColorB;
  uniform float uOpacity;
  uniform float uQuality;
  varying vec2 vUv;
  varying float vProgress;
  varying vec3 vNormal;
  varying vec3 vWorldPos;

  void main() {
    // Color gradient along ribbon
    float colorMix = vProgress + sin(uTime * 0.2 + vProgress * 3.0) * 0.08;
    vec3 baseColor = mix(uColorA, uColorB, clamp(colorMix, 0.0, 1.0));

    // Fresnel rim
    vec3 viewDir = normalize(cameraPosition - vWorldPos);
    float fresnel = 1.0 - abs(dot(vNormal, viewDir));
    fresnel = pow(fresnel, 3.0);

    vec3 finalColor = baseColor + baseColor * fresnel * 0.4;

    // Gentle energy pulse
    if (uQuality > 0.5) {
      float pulse = sin(vProgress * 15.0 - uTime * 1.5) * 0.5 + 0.5;
      finalColor += baseColor * pulse * 0.06;
    }

    // Cross-section shading
    float crossFade = abs(vUv.y - 0.5) * 2.0;
    finalColor *= 1.0 - crossFade * 0.25;

    // Fade at tips
    float tipFade = smoothstep(0.0, 0.08, vProgress) * smoothstep(1.0, 0.92, vProgress);

    float alpha = uOpacity * tipFade * uScrollFade;
    gl_FragColor = vec4(finalColor, alpha);
  }
`;

// ---------------------------------------------------------------------------
// Single speaker ribbon
// ---------------------------------------------------------------------------
function SpeakerRibbon({
  speakerIndex,
  colorA,
  colorB,
  scrollFade,
  quality,
}: {
  speakerIndex: number;
  colorA: THREE.Color;
  colorB: THREE.Color;
  scrollFade: React.MutableRefObject<number>;
  quality: QualityTier;
}) {
  const materialRef = useRef<THREE.ShaderMaterial>(null);

  const segments = quality === 'high' ? 180 : quality === 'medium' ? 100 : 60;
  const radial = quality === 'high' ? 10 : 6;

  const curve = useMemo(() => generateSpeakerCurve(speakerIndex, 36), [speakerIndex]);
  const geometry = useMemo(
    () => new THREE.TubeGeometry(curve, segments, 0.055, radial, false),
    [curve, segments, radial],
  );

  const uniforms = useMemo(
    () => ({
      uTime: { value: 0 },
      uScrollFade: { value: 1 },
      uColorA: { value: colorA },
      uColorB: { value: colorB },
      uOpacity: { value: 0.3 },
      uQuality: { value: quality === 'high' ? 1.0 : quality === 'medium' ? 0.5 : 0.0 },
    }),
    [colorA, colorB, quality],
  );

  useFrame((state) => {
    if (!materialRef.current) return;
    materialRef.current.uniforms.uTime.value = state.clock.elapsedTime;
    materialRef.current.uniforms.uScrollFade.value = scrollFade.current;
  });

  return (
    <mesh geometry={geometry}>
      <shaderMaterial
        ref={materialRef}
        vertexShader={ribbonVertexShader}
        fragmentShader={ribbonFragmentShader}
        uniforms={uniforms}
        transparent
        side={THREE.DoubleSide}
        depthWrite={false}
      />
    </mesh>
  );
}

// ---------------------------------------------------------------------------
// Particle dust
// ---------------------------------------------------------------------------
function ConversationParticles({
  scrollFade,
  count,
}: {
  scrollFade: React.MutableRefObject<number>;
  count: number;
}) {
  const pointsRef = useRef<THREE.Points>(null);

  const { positions, speeds, offsets } = useMemo(() => {
    const pos = new Float32Array(count * 3);
    const spd = new Float32Array(count);
    const off = new Float32Array(count);
    for (let i = 0; i < count; i++) {
      pos[i * 3] = (Math.random() - 0.5) * 2.5;
      pos[i * 3 + 1] = (Math.random() - 0.5) * 5;
      pos[i * 3 + 2] = (Math.random() - 0.5) * 2;
      spd[i] = 0.15 + Math.random() * 0.5;
      off[i] = Math.random() * Math.PI * 2;
    }
    return { positions: pos, speeds: spd, offsets: off };
  }, [count]);

  const bufferGeo = useMemo(() => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute(new Float32Array(positions), 3));
    return geo;
  }, [positions]);

  useFrame((state) => {
    if (!pointsRef.current) return;
    const posAttr = pointsRef.current.geometry.getAttribute('position') as THREE.BufferAttribute;
    const t = state.clock.elapsedTime;

    for (let i = 0; i < count; i++) {
      posAttr.setXYZ(
        i,
        positions[i * 3] + Math.sin(t * speeds[i] * 0.4 + offsets[i]) * 0.1,
        positions[i * 3 + 1] + Math.cos(t * speeds[i] * 0.25 + offsets[i]) * 0.08,
        positions[i * 3 + 2] + Math.sin(t * speeds[i] * 0.3 + offsets[i] * 1.3) * 0.06,
      );
    }
    posAttr.needsUpdate = true;

    const mat = pointsRef.current.material as THREE.PointsMaterial;
    mat.opacity = 0.12 * scrollFade.current;
  });

  return (
    <points ref={pointsRef} geometry={bufferGeo}>
      <pointsMaterial
        size={0.018}
        color="#8899bb"
        transparent
        opacity={0.12}
        sizeAttenuation
        depthWrite={false}
        blending={THREE.AdditiveBlending}
      />
    </points>
  );
}

// ---------------------------------------------------------------------------
// Scene — desaturated, muted speaker colors
// ---------------------------------------------------------------------------
function TopologyScene({
  scrollFade,
  quality,
}: {
  scrollFade: React.MutableRefObject<number>;
  quality: QualityTier;
}) {
  const groupRef = useRef<THREE.Group>(null);

  // Muted, desaturated — like fog, not neon
  const colorA1 = useMemo(() => new THREE.Color('#2a5a8f'), []);
  const colorA2 = useMemo(() => new THREE.Color('#4a7aaf'), []);
  const colorB1 = useMemo(() => new THREE.Color('#6a4a8f'), []);
  const colorB2 = useMemo(() => new THREE.Color('#8a5a9f'), []);

  useFrame((state) => {
    if (!groupRef.current) return;
    // Very slow, very subtle rotation
    groupRef.current.rotation.y = Math.sin(state.clock.elapsedTime * 0.1) * 0.04;
  });

  const particleCount = quality === 'high' ? 200 : quality === 'medium' ? 80 : 0;

  return (
    <group ref={groupRef}>
      <SpeakerRibbon speakerIndex={0} colorA={colorA1} colorB={colorA2} scrollFade={scrollFade} quality={quality} />
      <SpeakerRibbon speakerIndex={1} colorA={colorB1} colorB={colorB2} scrollFade={scrollFade} quality={quality} />
      {particleCount > 0 && <ConversationParticles scrollFade={scrollFade} count={particleCount} />}
    </group>
  );
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------
function SceneCamera() {
  const { camera } = useThree();
  useEffect(() => {
    camera.position.set(0, 0, 7);
    camera.lookAt(0, 0, 0);
  }, [camera]);
  return null;
}

// ---------------------------------------------------------------------------
// Static fallback
// ---------------------------------------------------------------------------
function StaticFallback() {
  return (
    <div
      className="absolute inset-0 pointer-events-none"
      style={{
        background:
          'radial-gradient(ellipse at 45% 30%, rgba(42,90,143,0.07) 0%, transparent 55%), ' +
          'radial-gradient(ellipse at 55% 40%, rgba(106,74,143,0.05) 0%, transparent 55%)',
      }}
    />
  );
}

// ---------------------------------------------------------------------------
// Exported component — absolutely positioned within hero, NOT fixed
// ---------------------------------------------------------------------------
export function ConversationTopology() {
  const scrollFade = useRef(1);
  const [quality, setQuality] = useState<QualityTier>('fallback');
  const [mounted, setMounted] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setMounted(true);
    setQuality(detectQualityTier());
  }, []);

  // Track how much of the hero is still visible — fade canvas as it scrolls away
  const handleScroll = useCallback(() => {
    if (!containerRef.current) return;
    const rect = containerRef.current.getBoundingClientRect();
    const heroBottom = rect.bottom;
    const viewportH = window.innerHeight;
    // 1.0 when hero fully visible, 0.0 when hero scrolled out
    const visibility = Math.max(0, Math.min(1, heroBottom / viewportH));
    // Ease the fade
    scrollFade.current = visibility * visibility;
  }, []);

  useEffect(() => {
    window.addEventListener('scroll', handleScroll, { passive: true });
    handleScroll();
    return () => window.removeEventListener('scroll', handleScroll);
  }, [handleScroll]);

  if (!mounted) return null;
  if (quality === 'fallback') return <StaticFallback />;

  return (
    <div
      ref={containerRef}
      className="absolute inset-0 pointer-events-none overflow-hidden"
      style={{ zIndex: 0, height: '100%' }}
      aria-hidden="true"
    >
      <Canvas
        gl={{
          alpha: true,
          antialias: quality === 'high',
          powerPreference: 'high-performance',
        }}
        dpr={quality === 'high' ? [1, 2] : [1, 1.5]}
        performance={{ min: 0.5 }}
        style={{ background: 'transparent', pointerEvents: 'none' }}
      >
        <SceneCamera />
        <TopologyScene scrollFade={scrollFade} quality={quality} />
      </Canvas>
    </div>
  );
}
