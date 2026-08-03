#version 300 es
precision highp float;

in vec3 v_normal;
in vec4 v_color;

/// Zero for the shapes drawn as lines, which carry no surface to light.
uniform float u_lit;

out vec4 o_color;

void main() {
	vec3 normal = normalize(v_normal);
	// One key light and a hemisphere for whatever is turned away from it, so a box reads as solid
	// without the scene carrying any lighting of its own.
	float key = max(dot(normal, normalize(vec3(0.4, 1.0, 0.3))), 0.0);
	float light = mix(1.0, 0.45 + 0.55 * key, u_lit);
	o_color = vec4(v_color.rgb * light, v_color.a);
}
