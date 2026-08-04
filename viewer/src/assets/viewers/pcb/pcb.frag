#version 300 es
precision highp float;

in vec3 v_position;
in vec3 v_normal;
in vec4 v_color;

uniform vec3 u_eye;

out vec4 out_color;

void main() {
    vec3 normal = v_normal;
    float len = length(normal);
    if (len > 1e-6) {
        normal /= len;
    } else {
        normal = vec3(0.0, 1.0, 0.0);
    }
    if (!gl_FrontFacing) {
        normal = -normal;
    }
    const vec3 KEY = vec3(-0.45, 0.82, 0.36);
    const vec3 SKY = vec3(0.20, 0.23, 0.29);
    const vec3 GROUND = vec3(0.09, 0.085, 0.08);
    vec3 view = normalize(u_eye - v_position);
    float key = max(dot(normal, KEY), 0.0);
    vec3 ambient = mix(GROUND, SKY, normal.y * 0.5 + 0.5);
    vec3 reflect_key = reflect(-KEY, normal);
    float specular = pow(max(dot(reflect_key, view), 0.0), 18.0) * 0.18;
    vec3 lit = v_color.rgb * (0.40 * ambient + 0.75 * key) + specular;
    out_color = vec4(lit, v_color.a);
}
