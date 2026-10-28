(function () {
  'use strict';

  var canvas = document.getElementById('pool-canvas');
  if (!canvas || !window.THREE) return;

  var THREE = window.THREE;
  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  var renderer;
  try {
    renderer = new THREE.WebGLRenderer({
      canvas: canvas,
      alpha: true,
      antialias: true,
      powerPreference: 'low-power'
    });
  } catch (error) {
    return;
  }

  var scene = new THREE.Scene();
  var camera = new THREE.PerspectiveCamera(50, 1, 0.1, 120);
  camera.position.set(0, 0, 26);
  var group = new THREE.Group();
  scene.add(group);

  var small = window.innerWidth < 720;
  var count = small ? 55 : 100;
  var positions = new Float32Array(count * 3);
  var colors = new Float32Array(count * 3);
  var palette = [0x5eead4, 0x4ade80, 0x38bdf8, 0xfbbf24].map(function (hex) {
    return new THREE.Color(hex);
  });

  for (var i = 0; i < count; i++) {
    var i3 = i * 3;
    positions[i3] = (Math.random() * 2 - 1) * 20;
    positions[i3 + 1] = (Math.random() * 2 - 1) * 12;
    positions[i3 + 2] = (Math.random() * 2 - 1) * 10;
    var color = palette[i % palette.length];
    colors[i3] = color.r;
    colors[i3 + 1] = color.g;
    colors[i3 + 2] = color.b;
  }

  var pointGeometry = new THREE.BufferGeometry();
  pointGeometry.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  pointGeometry.setAttribute('color', new THREE.BufferAttribute(colors, 3));

  var spriteCanvas = document.createElement('canvas');
  spriteCanvas.width = 64;
  spriteCanvas.height = 64;
  var spriteContext = spriteCanvas.getContext('2d');
  var gradient = spriteContext.createRadialGradient(32, 32, 0, 32, 32, 32);
  gradient.addColorStop(0, 'rgba(255,255,255,1)');
  gradient.addColorStop(0.35, 'rgba(255,255,255,.8)');
  gradient.addColorStop(1, 'rgba(255,255,255,0)');
  spriteContext.fillStyle = gradient;
  spriteContext.fillRect(0, 0, 64, 64);
  var sprite = new THREE.CanvasTexture(spriteCanvas);

  var pointMaterial = new THREE.PointsMaterial({
    size: 0.55,
    map: sprite,
    vertexColors: true,
    transparent: true,
    opacity: 0.9,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    sizeAttenuation: true
  });
  group.add(new THREE.Points(pointGeometry, pointMaterial));

  var linePositions = [];
  var maxDistance = 6;
  for (var a = 0; a < count; a++) {
    for (var b = a + 1; b < count; b++) {
      var dx = positions[a * 3] - positions[b * 3];
      var dy = positions[a * 3 + 1] - positions[b * 3 + 1];
      var dz = positions[a * 3 + 2] - positions[b * 3 + 2];
      if (dx * dx + dy * dy + dz * dz < maxDistance * maxDistance) {
        linePositions.push(
          positions[a * 3], positions[a * 3 + 1], positions[a * 3 + 2],
          positions[b * 3], positions[b * 3 + 1], positions[b * 3 + 2]
        );
      }
    }
  }
  var lineGeometry = new THREE.BufferGeometry();
  lineGeometry.setAttribute('position', new THREE.Float32BufferAttribute(linePositions, 3));
  group.add(new THREE.LineSegments(lineGeometry, new THREE.LineBasicMaterial({
    color: 0x2dd4bf,
    transparent: true,
    opacity: 0.12,
    blending: THREE.AdditiveBlending,
    depthWrite: false
  })));

  var ringMaterial = new THREE.MeshBasicMaterial({
    color: 0x5eead4,
    transparent: true,
    opacity: 0.16,
    depthWrite: false
  });
  var outerRing = new THREE.Mesh(new THREE.TorusGeometry(9.5, 0.025, 4, 120), ringMaterial);
  outerRing.rotation.x = Math.PI / 2.3;
  var innerRing = new THREE.Mesh(new THREE.TorusGeometry(7.2, 0.02, 4, 100), new THREE.MeshBasicMaterial({
    color: 0x38bdf8,
    transparent: true,
    opacity: 0.13,
    depthWrite: false
  }));
  innerRing.rotation.x = Math.PI / 2.3;
  innerRing.rotation.y = 0.6;
  group.add(outerRing, innerRing);

  var pointer = { x: 0, y: 0 };
  if (!reduceMotion && window.matchMedia('(pointer: fine)').matches) {
    window.addEventListener('pointermove', function (event) {
      pointer.x = (event.clientX / window.innerWidth) * 2 - 1;
      pointer.y = (event.clientY / window.innerHeight) * 2 - 1;
    }, { passive: true });
  }

  function resize() {
    var width = canvas.clientWidth || window.innerWidth;
    var height = canvas.clientHeight || window.innerHeight;
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
    renderer.setSize(width, height, false);
    camera.aspect = width / Math.max(height, 1);
    camera.updateProjectionMatrix();
  }
  window.addEventListener('resize', resize, { passive: true });
  resize();

  function render() {
    group.rotation.y = 0.4;
    group.rotation.x = 0.1;
    renderer.render(scene, camera);
  }

  if (reduceMotion) {
    render();
    return;
  }

  var visible = true;
  if ('IntersectionObserver' in window) {
    new IntersectionObserver(function (entries) {
      visible = entries[0].isIntersecting;
    }, { threshold: 0 }).observe(canvas);
  }

  var clock = new THREE.Clock();
  var elapsed = 0;
  function frame() {
    window.requestAnimationFrame(frame);
    if (!visible || document.hidden) return;
    elapsed += Math.min(clock.getDelta(), 0.05);
    group.rotation.y += 0.0008;
    group.rotation.x = Math.sin(elapsed * 0.12) * 0.05;
    camera.position.x += (pointer.x * 2 - camera.position.x) * 0.03;
    camera.position.y += (-pointer.y * 1.4 - camera.position.y) * 0.03;
    camera.lookAt(0, 0, 0);
    renderer.render(scene, camera);
  }
  frame();
})();
