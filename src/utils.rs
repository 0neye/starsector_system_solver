

pub fn calculate_formula(formula: &str, size: u32) -> f32 {
    let formula = formula.replace("size", &size.to_string());
    let tokens: Vec<&str> = formula.split_whitespace().collect();
    
    fn evaluate(tokens: &[&str]) -> f32 {
        let mut stack: Vec<f32> = Vec::new();
        let mut op = "+";
        
        for &token in tokens {
            match token {
                "(" => continue,
                ")" => continue,
                "+" | "-" | "*" | "/" => op = token,
                _ => {
                    let num: f32 = token.parse().unwrap_or(0.0);
                    match op {
                        "+" => stack.push(num),
                        "-" => stack.push(-num),
                        "*" => *stack.last_mut().unwrap() *= num,
                        "/" => *stack.last_mut().unwrap() /= if num != 0.0 { num } else { 1.0 },
                        _ => stack.push(num),
                    }
                }
            }
        }
        stack.iter().sum()
    }

    evaluate(&tokens)
}